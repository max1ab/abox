use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
#[allow(dead_code)]
mod common;
const FIRST: &str = "019a4000-0000-7000-8000-000000000001";
const SECOND: &str = "019a4000-0000-7000-8000-000000000002";
fn command(root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut c);
    c.env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("AGENT_MAIL_SERVER")
        .env("CODEX_HOME", root.join("profile"))
        .env("CLAUDE_CONFIG_DIR", root.join("claude-profile"))
        .env("AGENT_MAIL_CODEX", root.join("absent-codex"))
        .current_dir(root)
        .arg("--data-dir")
        .arg(root)
        .args(args);
    c
}
fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn bind(root: &Path, team: &str, name: &str, id: &str) {
    ok(command(
        root,
        &["join", team, "--create", "--name", name, "--type", "codex"],
    )
    .output()
    .unwrap());
    ok(command(
        root,
        &[
            "bind",
            team,
            "--name",
            name,
            "--type",
            "codex",
            "--session-id",
            id,
        ],
    )
    .output()
    .unwrap());
}
fn send(root: &Path, team: Option<&str>) -> Output {
    let mut c = command(root, &["send", "reviewer", "--file", "body"]);
    c.env("CODEX_THREAD_ID", FIRST);
    if let Some(team) = team {
        c.args(["--team", team]);
    }
    c.output().unwrap()
}
#[test]
fn public_help_has_one_clear_entry_for_each_operation() {
    let t = tempfile::tempdir().unwrap();
    let help = ok(command(t.path(), &["--help"]).output().unwrap());
    for name in [
        "setup", "start", "send", "inbox", "list", "history", "status", "service", "debug",
    ] {
        assert!(
            help.lines()
                .any(|l| l.trim_start().starts_with(&format!("{name} "))),
            "{help}"
        );
    }
    for name in [
        "agmsg",
        "join",
        "bind",
        "daemon",
        "notify",
        "sync",
        "hook",
        "directory",
    ] {
        assert!(
            !help
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{name} "))),
            "{help}"
        );
    }
    ok(command(t.path(), &["board", "post", "--help"])
        .output()
        .unwrap());
    ok(command(t.path(), &["board", "list", "--help"])
        .output()
        .unwrap());
}
#[test]
fn send_resolves_the_session_and_refuses_ambiguous_or_foreign_bindings() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    for team in ["dev", "other"] {
        bind(r, team, "worker", FIRST);
        bind(r, team, "reviewer", SECOND);
    }
    fs::write(r.join("body"), "please review").unwrap();
    let ambiguous = send(r, None);
    assert!(!ambiguous.status.success());
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("--team"));
    ok(send(r, Some("dev")));
    let history = ok(command(r, &["history", "--team", "dev"]).output().unwrap());
    let mail: Value = serde_json::from_str(history.trim()).unwrap();
    assert_eq!(mail["from"], "worker");
    assert_eq!(mail["body"], "please review");
    assert!(
        ok(command(r, &["history", "--team", "other"])
            .output()
            .unwrap())
        .is_empty()
    );
    let mut foreign = command(r, &["send", "reviewer", "--team", "dev", "--file", "body"]);
    foreign
        .env("CODEX_THREAD_ID", FIRST)
        .env("CODEX_HOME", r.join("different-profile"));
    assert!(!foreign.output().unwrap().status.success());
    let listing = ok(command(r, &["list"]).output().unwrap());
    assert_eq!(listing.lines().count(), 4);
    let listing = ok(command(r, &["list", "--team", "dev"]).output().unwrap());
    assert_eq!(listing.lines().count(), 2);
    let inbox = ok(command(r, &["inbox"])
        .env("CODEX_THREAD_ID", SECOND)
        .output()
        .unwrap());
    assert!(inbox.contains("please review"));
    let inbox = ok(command(r, &["inbox"])
        .env("CODEX_THREAD_ID", SECOND)
        .output()
        .unwrap());
    assert!(!inbox.contains("please review"));
}
#[test]
fn local_send_works_when_remote_send_in_the_same_team_is_rejected() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    bind(r, "dev", "worker", FIRST);
    bind(r, "dev", "reviewer", SECOND);
    fs::write(r.join("body"), "please review").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let path = r.join("teams/dev/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let server_id = uuid::Uuid::now_v7().to_string();
    let team_id = config["team_id"].as_str().unwrap().to_owned();
    config["agents"]["remote"] = serde_json::json!({
        "member_id": uuid::Uuid::now_v7().to_string(), "registrations": []
    });
    config["remote_binding"] = serde_json::json!({
        "endpoint": format!("http://{}", listener.local_addr().unwrap()),
        "server_instance_id": server_id, "remote_team_id": team_id,
        "protocol_version": 1, "cipher_profile": "none", "disconnected_at": null
    });
    fs::write(path, config.to_string()).unwrap();

    ok(send(r, Some("dev")));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let inbox = command(r, &["inbox"])
        .env("CODEX_THREAD_ID", SECOND)
        .output()
        .unwrap();
    assert!(ok(inbox).contains("please review"));
    let history = || ok(command(r, &["history", "--team", "dev"]).output().unwrap());
    let before = history();

    let relay = thread::spawn(move || {
        let capabilities = serde_json::json!({
            "server_instance_id": server_id, "team_id": team_id, "protocol_version": 1,
            "cipher_profile": "none", "accepted_envelope_versions": [1],
            "write_allowed_ciphers": ["none"]
        })
        .to_string();
        for (status, body) in [
            ("503 Service Unavailable", ""),
            ("200 OK", capabilities.as_str()),
        ] {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing relay check");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = BufReader::new(&stream);
            let mut line = String::new();
            request.read_line(&mut line).unwrap();
            assert!(line.starts_with("GET /v1/capabilities "));
            loop {
                line.clear();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            write!(stream, "HTTP/1.1 {status}\r\nAgmsg-Protocol-Version: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let send_remote = || {
        command(r, &["send", "remote", "--team", "dev", "--file", "body"])
            .env("CODEX_THREAD_ID", FIRST)
            .env("NO_PROXY", "*")
            .output()
            .unwrap()
    };
    let failed = send_remote();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("HTTP 503"));
    assert_eq!(history(), before);
    assert!(ok(send_remote()).contains("Queued for remote delivery"));
    relay.join().unwrap();
    let queued = history();
    assert_eq!(queued.lines().count(), before.lines().count() + 1);

    // The listener is now closed. A transport error also leaves the outbox unchanged.
    let disconnected = send_remote();
    assert!(!disconnected.status.success());
    assert!(String::from_utf8_lossy(&disconnected.stderr).contains("remote request failed"));
    assert_eq!(history(), queued);
}

#[test]
fn invalid_start_does_not_register_an_identity_and_status_does_not_start_services() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let output = command(
        r,
        &[
            "start",
            "--team",
            "dev",
            "--name",
            "worker",
            "--create",
            "--type",
            "codex",
            "--session-id",
            "bogus",
            "--no-login",
        ],
    )
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(!r.join("teams").exists());
    bind(r, "dev", "worker", FIRST);
    let status = ok(command(r, &["status"])
        .env("CODEX_THREAD_ID", FIRST)
        .output()
        .unwrap());
    let status: Value = serde_json::from_str(status.trim()).unwrap();
    assert_eq!(status["service"]["running"], false);
    assert_eq!(status["current_session"]["identities"][0]["name"], "worker");
    assert!(!r.join("run/notifications.db").exists());
    let conflicting = command(r, &["send", "reviewer", "--stdin"])
        .env("CODEX_THREAD_ID", FIRST)
        .env("CLAUDE_CODE_SESSION_ID", SECOND)
        .output()
        .unwrap();
    assert!(!conflicting.status.success());
    assert!(String::from_utf8_lossy(&conflicting.stderr).contains("--type"));
}
#[test]
fn setup_is_repeatable_and_preserves_different_skills_before_changing_hooks() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let args = ["setup", "--type", "codex", "--config", "hooks.json"];
    ok(command(r, &args).output().unwrap());
    let hooks = fs::read(r.join("hooks.json")).unwrap();
    ok(command(r, &args).output().unwrap());
    assert_eq!(hooks, fs::read(r.join("hooks.json")).unwrap());
    let skill = r.join(".agents/skills/agent-mail/SKILL.md");
    assert!(
        fs::read_to_string(&skill)
            .unwrap()
            .contains("agent-mail start")
    );
    fs::write(&skill, "my own skill").unwrap();
    assert!(!command(r, &args).output().unwrap().status.success());
    assert_eq!(hooks, fs::read(r.join("hooks.json")).unwrap());
    assert_eq!(fs::read_to_string(skill).unwrap(), "my own skill");
}
#[test]
fn start_combines_registration_binding_and_idempotent_service_start() {
    struct Stop<'a>(&'a Path);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            let _ = command(self.0, &["service", "stop"]).output();
        }
    }
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    let _cleanup = Stop(r);
    let args = [
        "start",
        "--team",
        "dev",
        "--name",
        "worker",
        "--create",
        "--no-login",
    ];
    ok(command(r, &args)
        .env("CODEX_THREAD_ID", FIRST)
        .output()
        .unwrap());
    let before: Value =
        serde_json::from_str(ok(command(r, &["service", "status"]).output().unwrap()).trim())
            .unwrap();
    assert_eq!(before["running"], true);
    ok(command(r, &args)
        .env("CODEX_THREAD_ID", FIRST)
        .output()
        .unwrap());
    let after: Value =
        serde_json::from_str(ok(command(r, &["service", "status"]).output().unwrap()).trim())
            .unwrap();
    assert_eq!(before["pid"], after["pid"]);
    let record = r.join("run/role-session.dev__worker");
    let previous = fs::read(&record).unwrap();
    assert!(
        !command(r, &args)
            .env("CODEX_THREAD_ID", SECOND)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&record).unwrap(), previous);
    let mut replace = command(r, &args);
    replace.arg("--replace").env("CODEX_THREAD_ID", SECOND);
    ok(replace.output().unwrap());
    assert!(fs::read_to_string(record).unwrap().contains(SECOND));
    ok(command(r, &["service", "stop"]).output().unwrap());
}

#[test]
fn claude_runtime_identity_works_without_a_messaging_socket() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    for name in ["worker", "reviewer"] {
        ok(command(
            r,
            &[
                "join",
                "dev",
                "--create",
                "--name",
                name,
                "--type",
                "claude-code",
            ],
        )
        .output()
        .unwrap());
    }
    ok(command(
        r,
        &["bind", "dev", "--name", "worker", "--type", "claude-code"],
    )
    .env("CLAUDE_CODE_SESSION_ID", FIRST)
    .output()
    .unwrap());
    fs::write(r.join("body"), "claude task").unwrap();
    ok(command(r, &["send", "reviewer", "--file", "body"])
        .env("CLAUDE_CODE_SESSION_ID", FIRST)
        .output()
        .unwrap());
    let mail: Value = serde_json::from_str(
        ok(command(r, &["history"])
            .env("CLAUDE_CODE_SESSION_ID", FIRST)
            .output()
            .unwrap())
        .trim(),
    )
    .unwrap();
    assert_eq!(mail["from"], "worker");
}

#[test]
fn board_uses_current_identity_and_keeps_explicit_remote_reads_available() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    bind(r, "dev", "worker", FIRST);
    fs::write(r.join("body"), "current focus\n").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for method in ["POST", "GET"] {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "client never contacted fake board"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut bytes = vec![];
            let mut buf = [0u8; 1024];
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]);
                    let len: usize = header
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with(method), "{request}");
            let body = if method == "POST" {
                let mut post: Value =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                assert_eq!(post["team"], "dev");
                assert_eq!(post["author"], "worker");
                assert_eq!(post["body"], "current focus\n");
                post["id"] = 1.into();
                post["created_at"] = "2026-11-06T00:00:00Z".into();
                post.to_string()
            } else {
                assert!(request.lines().next().unwrap().contains("team=dev"));
                "[]".into()
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    ok(command(
        r,
        &["--server", &url, "board", "post", "--body-file", "body"],
    )
    .env("NO_PROXY", "*")
    .env("CODEX_THREAD_ID", FIRST)
    .output()
    .unwrap());
    ok(
        command(r, &["--server", &url, "board", "list", "--team", "dev"])
            .env("NO_PROXY", "*")
            .output()
            .unwrap(),
    );
    server.join().unwrap();
}
