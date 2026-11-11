use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::Command,
    thread,
    time::Duration,
};
#[allow(dead_code)]
mod common;
fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut c);
    c.env("NO_PROXY", "*")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("AGENT_MAIL_SERVER")
        .env("CODEX_HOME", root.join("codex-profile"))
        .env("AGENT_MAIL_BASH", root.join("no-bash"))
        .arg("--data-dir")
        .arg(root)
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn fixture() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("VERSION"), "1.7.0\n").unwrap();
    assert!(
        cli(
            t.path(),
            &[
                "join", "dev", "--create", "--name", "worker", "--type", "codex"
            ]
        )
        .status
        .success()
    );
    t
}
#[test]
fn connect_registers_stable_ids_and_reconciles_an_existing_remote_registration() {
    for (conflict, named) in [(false, false), (true, false), (false, true)] {
        let t = fixture();
        let r = t.path();
        let path = r.join("teams/dev/config.json");
        let cfg: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let expected = cfg.clone();
        let server = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            let mut count = 0;
            while count < if conflict || named { 3 } else { 1 } {
                assert!(
                    std::time::Instant::now() < deadline,
                    "client did not finish protocol"
                );
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut b = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let n = match stream.read(&mut buf) {
                        Ok(n) => n,
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                            ) =>
                        {
                            break;
                        }
                        Err(e) => panic!("{e}"),
                    };
                    if n == 0 {
                        break;
                    }
                    b.extend_from_slice(&buf[..n]);
                    if let Some(end) = b.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&b[..end]);
                        let length = headers
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|n| n.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if b.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                if b.is_empty() {
                    continue;
                }
                let source = String::from_utf8(b).unwrap();
                let line = source.lines().next().unwrap();
                count += 1;
                let mut body = json!({"protocol_version":1,"server_instance_id":"019a4000-0000-7000-8000-000000000001","team_id":expected["team_id"],"team_name":"dev","cipher_profile":"none","accepted_envelope_versions":[1],"write_allowed_ciphers":["none"],"min_available_seq":"0"});
                body["current_seq"] = json!("0");
                let publish = if named {
                    "POST /v1/agent-mail/teams "
                } else {
                    "POST /v1/connect "
                };
                let code = if line.starts_with(publish) {
                    let payload: Value =
                        serde_json::from_str(source.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(payload["team_id"], expected["team_id"]);
                    assert_eq!(
                        payload["members"][0]["member_id"],
                        expected["agents"]["worker"]["member_id"]
                    );
                    assert_eq!(payload["cipher_profile"], "none");
                    if conflict { 409 } else { 200 }
                } else if line.starts_with("GET /v1/members ") {
                    body["members"] = json!([{"name":"worker","member_id":expected["agents"]["worker"]["member_id"]}]);
                    200
                } else {
                    assert!(line.starts_with(&format!(
                        "GET /v1/teams/{} ",
                        expected["team_id"].as_str().unwrap()
                    )));
                    200
                };
                let body = body.to_string();
                write!(stream,"HTTP/1.1 {code} OK\r\nAgmsg-Protocol-Version: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let output = if named {
            let started = cli(
                r,
                &[
                    "--server",
                    &endpoint,
                    "start",
                    "--team",
                    "dev",
                    "--name",
                    "worker",
                    "--create",
                    "--type",
                    "codex",
                    "--session-id",
                    "019a4000-0000-7000-8000-000000000005",
                    "--no-login",
                ],
            );
            let _ = cli(r, &["service", "stop"]);
            started
        } else {
            cli(r, &["--server", &endpoint, "team", "connect", "dev"])
        };
        server.join().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let after: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(after["team_id"], cfg["team_id"]);
        assert_eq!(after["agents"], cfg["agents"]);
        assert_eq!(after["remote_binding"]["remote_team_id"], cfg["team_id"]);
        assert_eq!(
            after["remote_binding"]["history_boundary"].is_object(),
            named
        );
    }
}
