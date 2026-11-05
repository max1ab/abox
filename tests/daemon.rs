use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
    time::{Duration, Instant},
};
#[allow(dead_code)]
mod common;
fn cli(root: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut c);
    c.env("AGENT_MAIL_CODEX", root.join("fake-codex"))
        .env("CODEX_HOME", root)
        .current_dir(root)
        .arg("--agmsg-dir")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}
fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn status(r: &Path) -> Value {
    serde_json::from_str(ok(cli(r, &["daemon", "status"])).trim()).unwrap()
}
struct Stop<'a>(&'a Path);
impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = cli(self.0, &["daemon", "stop"]);
    }
}
#[test]
fn status_does_not_create_runtime_state() {
    let t = tempfile::tempdir().unwrap();
    assert_eq!(status(t.path())["running"], false);
    assert!(!t.path().join("run").exists());
}
#[cfg(unix)]
#[test]
fn one_detached_daemon_notifies_local_mail_even_when_a_remote_team_is_offline() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("VERSION"), "1.7.0\n").unwrap();
    fs::write(r.join("fake-codex"),"#!/bin/sh\nprintf 'called\\n' >> \"$CODEX_HOME/calls\"\nprintf 'Queued message 00000000-0000-4000-8000-000000000001 for thread %s.\\n' \"$3\"\n").unwrap();
    fs::set_permissions(r.join("fake-codex"), fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["sender", "receiver"] {
        ok(cli(
            r,
            &["join", "dev", "--create", "--name", name, "--type", "codex"],
        ));
    }
    ok(cli(
        r,
        &[
            "bind",
            "dev",
            "--name",
            "receiver",
            "--type",
            "codex",
            "--session-id",
            "019a4000-0000-7000-8000-000000000001",
        ],
    ));
    ok(cli(
        r,
        &[
            "join", "offline", "--create", "--name", "worker", "--type", "codex",
        ],
    ));
    let path = r.join("teams/offline/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["remote_binding"] = json!({"protocol_version":1,"cipher_profile":"none","disconnected_at":null,"endpoint":"http://127.0.0.1:9","server_instance_id":"019a4000-0000-7000-8000-000000000002","remote_team_id":"019a4000-0000-7000-8000-000000000003"});
    fs::write(path, config.to_string()).unwrap();
    let _cleanup = Stop(r);
    ok(cli(r, &["daemon", "start", "--no-login"]));
    let before = status(r);
    assert_eq!(before["running"], true);
    assert_eq!(before["supervised"], false);
    ok(cli(r, &["daemon", "start", "--no-login"]));
    assert_eq!(status(r)["pid"], before["pid"]);
    assert!(!cli(r, &["daemon", "run"]).status.success());
    fs::write(r.join("body"), "local task").unwrap();
    ok(cli(
        r,
        &[
            "agmsg",
            "send",
            "dev",
            "--from",
            "sender",
            "--to",
            "receiver",
            "--body-file",
            "body",
        ],
    ));
    let deadline = Instant::now() + Duration::from_secs(6);
    while !r.join("calls").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        r.join("calls").exists(),
        "{}",
        fs::read_to_string(r.join("run/daemon/daemon.log")).unwrap_or_default()
    );
    assert!(status(r)["teams"].get("offline").is_some());
    ok(cli(r, &["daemon", "stop"]));
    assert_eq!(status(r)["running"], false);
    let log = fs::read_to_string(r.join("run/daemon/daemon.log")).unwrap();
    assert!(log.contains("daemon.start"));
    assert!(log.contains("daemon.exit"));
}
#[test]
fn old_agmsgd_ownership_blocks_a_second_notification_engine() {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("VERSION"), "1.7.0\n").unwrap();
    fs::create_dir(r.join("run")).unwrap();
    let db = rusqlite::Connection::open(r.join("run/install.db")).unwrap();
    db.execute_batch("CREATE TABLE daemon_intent(desired TEXT);INSERT INTO daemon_intent VALUES('on');CREATE TABLE daemon_owner(state TEXT);INSERT INTO daemon_owner VALUES('ready');").unwrap();
    let out = cli(r, &["daemon", "start", "--no-login"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("agmsgd"));
    assert!(!r.join("run/daemon/endpoint.json").exists());
    assert_eq!(
        db.query_row("SELECT desired FROM daemon_intent", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "on"
    );
}
