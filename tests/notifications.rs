#![cfg(unix)]
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
#[allow(dead_code)]
mod common;
const SESSION: &str = "019a4000-0000-7000-8000-000000000001";
fn cli(root: &Path, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut c);
    c.env("AGENT_MAIL_CODEX", root.join("fake-codex"))
        .env("CODEX_HOME", root)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .arg("--agmsg-dir")
        .arg(root)
        .current_dir(root)
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
fn fixture() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("VERSION"), "1.7.0\n").unwrap();
    fs::write(r.join("fake-codex"),"#!/bin/sh\nprintf '%s' \"$5\" > \"$CODEX_HOME/notice\"\nprintf 'called\\n' >> \"$CODEX_HOME/calls\"\nprintf 'Queued message 00000000-0000-4000-8000-000000000001 for thread %s.\\n' \"$3\"\n").unwrap();
    fs::set_permissions(r.join("fake-codex"), fs::Permissions::from_mode(0o700)).unwrap();
    for team in ["dev", "other"] {
        for name in ["sender", "receiver"] {
            ok(cli(
                r,
                &["join", team, "--create", "--name", name, "--type", "codex"],
            ));
        }
        ok(cli(
            r,
            &[
                "bind",
                team,
                "--name",
                "receiver",
                "--type",
                "codex",
                "--session-id",
                SESSION,
            ],
        ));
    }
    t
}
fn send(r: &Path, team: &str) {
    fs::write(r.join("body"), "new task").unwrap();
    ok(cli(
        r,
        &[
            "agmsg",
            "send",
            team,
            "--from",
            "sender",
            "--to",
            "receiver",
            "--body-file",
            "body",
        ],
    ));
}
fn scan(r: &Path) -> Value {
    serde_json::from_str(ok(cli(r, &["notify", "once"])).trim()).unwrap()
}
fn state(r: &Path) -> Vec<Value> {
    ok(cli(r, &["notify", "status"]))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
#[test]
fn one_reminder_covers_all_teams_until_the_reminder_itself_is_consumed() {
    let t = fixture();
    let r = t.path();
    send(r, "dev");
    send(r, "other");
    assert_eq!(scan(r)["submitted"], 1);
    let nonce = state(r)[0]["nonce"].as_str().unwrap().to_owned();
    send(r, "dev");
    assert_eq!(scan(r)["submitted"], 0);
    // Manual reads must not create another queue item while the original
    // reminder can still be waiting in the desktop app.
    for team in ["dev", "other"] {
        ok(cli(r, &["agmsg", "inbox", team, "receiver"]));
    }
    assert_eq!(scan(r)["submitted"], 0);
    send(r, "other");
    assert_eq!(scan(r)["submitted"], 0);
    let read = ok(cli(
        r,
        &[
            "inbox",
            "--type",
            "codex",
            "--session-id",
            SESSION,
            "--notification",
            &nonce,
        ],
    ));
    assert!(read.contains("other/receiver:"));
    assert_eq!(scan(r)["submitted"], 0);
    send(r, "dev");
    assert_eq!(scan(r)["submitted"], 1);
    assert_eq!(
        fs::read_to_string(r.join("calls")).unwrap().lines().count(),
        2
    );
}
#[test]
fn ambiguous_send_result_is_persistent_and_never_automatically_retried() {
    let t = fixture();
    let r = t.path();
    fs::write(
        r.join("fake-codex"),
        "#!/bin/sh\nprintf 'unexpected output\\n'\n",
    )
    .unwrap();
    send(r, "dev");
    assert_eq!(scan(r)["submitted"], 0);
    assert_eq!(state(r)[0]["state"], "unknown");
    send(r, "other");
    assert_eq!(scan(r)["submitted"], 0);
    assert_eq!(state(r).len(), 1);
}

fn session_inbox(root: &Path) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut c);
    c.env("CODEX_HOME", root)
        .env("CODEX_THREAD_ID", SESSION)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .arg("--data-dir")
        .arg(root)
        .arg("inbox")
        .output()
        .unwrap()
}

#[test]
fn short_notice_and_plain_inbox_acknowledge_the_session_without_extra_arguments() {
    let t = fixture();
    let r = t.path();
    send(r, "dev");
    send(r, "other");
    assert_eq!(scan(r)["submitted"], 1);
    let nonce = state(r)[0]["nonce"].as_str().unwrap().to_owned();
    assert_eq!(
        fs::read_to_string(r.join("notice")).unwrap(),
        format!("[agent-mail:{nonce}] New messages are waiting. Check your agent-mail inbox.")
    );
    let read = ok(session_inbox(r));
    assert!(read.contains("dev/receiver:"));
    assert!(read.contains("other/receiver:"));
    assert_eq!(read.matches("new task").count(), 2);
    assert_eq!(state(r)[0]["state"], "acknowledged");
    assert_eq!(scan(r)["submitted"], 0);
    send(r, "dev");
    assert_eq!(scan(r)["submitted"], 1);
    let pending = state(r)
        .into_iter()
        .find(|v| v["state"] == "accepted")
        .unwrap();
    assert_ne!(pending["nonce"], nonce);
    // A delayed old notice must not acknowledge the new reminder by mistake.
    ok(cli(
        r,
        &[
            "inbox",
            "--type",
            "codex",
            "--session-id",
            SESSION,
            "--notification",
            &nonce,
        ],
    ));
    assert!(
        state(r)
            .iter()
            .any(|v| v["nonce"] == pending["nonce"] && v["state"] == "accepted")
    );
    // Even an empty inbox can consume the actual pending session reminder.
    ok(session_inbox(r));
    assert!(state(r).iter().all(|v| v["state"] == "acknowledged"));
}

#[test]
fn unsuccessful_aggregate_read_keeps_the_pending_reminder() {
    let t = fixture();
    let r = t.path();
    send(r, "dev");
    assert_eq!(scan(r)["submitted"], 1);
    let binding = r.join("run/role-session.other__receiver");
    let original = fs::read_to_string(&binding).unwrap();
    fs::write(
        &binding,
        original
            .lines()
            .map(|line| {
                if line.starts_with("project=") {
                    format!("project={}\n", r.join("missing-project").display())
                } else {
                    format!("{line}\n")
                }
            })
            .collect::<String>(),
    )
    .unwrap();
    assert!(!session_inbox(r).status.success());
    assert_eq!(state(r)[0]["state"], "accepted");
    fs::write(binding, original).unwrap();
    assert!(ok(session_inbox(r)).contains("new task"));
    assert_eq!(state(r)[0]["state"], "acknowledged");
}
