use std::process::Command;

use serde_json::Value;

#[test]
fn doctor_reports_both_configuration_errors_and_returns_failure() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_abox"))
        .args(["--server", "invalid-url", "--agmsg-dir"])
        .arg(temp.path().join("missing"))
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let checks: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0]["component"], "mailbox");
    assert_eq!(checks[0]["ok"], false);
    assert!(
        checks[0]["detail"]
            .as_str()
            .unwrap()
            .contains("cannot open Abox data directory")
    );
    assert_eq!(checks[1]["component"], "server");
    assert_eq!(checks[1]["ok"], false);
    assert!(
        checks[1]["detail"]
            .as_str()
            .unwrap()
            .contains("invalid --server URL")
    );
}

#[test]
fn doctor_accepts_existing_relay_health_response() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let relay = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let length = socket.read(&mut request).unwrap();
        assert!(request[..length].starts_with(b"GET /v1/agent-mail/health "));
        let body = r#"{"service":"agent-mail","status":"ok"}"#;
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });

    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_abox"))
        .args(["--server", &url, "--data-dir"])
        .arg(temp.path().join("missing"))
        .arg("doctor")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let server: Value = serde_json::from_str(stdout.lines().nth(1).unwrap()).unwrap();
    assert_eq!(server["component"], "server");
    assert_eq!(server["ok"], true, "{server}");
    relay.join().unwrap();
}
