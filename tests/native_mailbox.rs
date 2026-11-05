//! Product behavior with no upstream scripts and an unusable Bash path.
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
#[allow(dead_code)]
mod common;

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut command);
    command
        .env("AGENT_MAIL_BASH", root.join("bash-must-not-run"))
        .arg("--agmsg-dir")
        .arg(root)
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}
fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("VERSION"), "v1.7.0\n").unwrap();
    for name in ["sender", "receiver", "outsider"] {
        ok(cli(
            temp.path(),
            &["join", "dev", "--create", "--name", name, "--type", "codex"],
        ));
    }
    temp
}
fn send(root: &Path, from: &str, to: &str, body: &str) {
    let file = root.join("body.txt");
    fs::write(&file, body).unwrap();
    ok(cli(
        root,
        &[
            "agmsg",
            "send",
            "dev",
            "--from",
            from,
            "--to",
            to,
            "--body-file",
            file.to_str().unwrap(),
        ],
    ));
}
fn messages(root: &Path, args: &[&str]) -> Vec<Value> {
    ok(cli(root, args))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
#[test]
fn registration_mail_and_queries_work_without_upstream_code() {
    let temp = fixture();
    let root = temp.path();
    assert!(!root.join("scripts").exists());
    let config_before = fs::read(root.join("teams/dev/config.json")).unwrap();
    ok(cli(
        root,
        &["join", "dev", "--name", "receiver", "--type", "codex"],
    ));
    assert_eq!(
        config_before,
        fs::read(root.join("teams/dev/config.json")).unwrap()
    );
    assert_eq!(messages(root, &["agmsg", "teams"])[0]["name"], "dev");
    let members = messages(root, &["agmsg", "members", "dev"]);
    assert_eq!(members.len(), 3);
    assert_eq!(members[1]["types"], serde_json::json!(["codex"]));
    send(root, "sender", "receiver", "中文\n'quotes' $(literal)\n\n");
    send(root, "sender", "outsider", "unrelated");
    assert_eq!(messages(root, &["agmsg", "messages", "dev"]).len(), 2);
    let filtered = messages(root, &["agmsg", "messages", "dev", "--agent", "receiver"]);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["body"], "中文\n'quotes' $(literal)");
    assert_eq!(
        messages(root, &["agmsg", "messages", "dev", "--agent", "absent"]).len(),
        0
    );
    assert_eq!(
        messages(root, &["agmsg", "messages", "dev", "--limit", "1"])[0]["body"],
        "unrelated"
    );
    let inbox = ok(cli(root, &["agmsg", "inbox", "dev", "receiver"]));
    assert!(inbox.contains("1 new message(s):"));
    assert_eq!(
        ok(cli(root, &["agmsg", "inbox", "dev", "receiver"])).trim(),
        "No new messages."
    );
    assert!(ok(cli(root, &["agmsg", "inbox", "dev", "outsider"])).contains("1 new message(s):"));
    assert_eq!(messages(root, &["agmsg", "messages", "dev"]).len(), 2);
    let db = rusqlite::Connection::open(root.join("db/messages.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM messages WHERE read_at IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}
#[test]
fn concurrent_joins_do_not_lose_members_and_conflicting_names_fail() {
    let temp = fixture();
    let root = temp.path();
    let children: Vec<_> = (0..6)
        .map(|n| {
            let mut c = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
            common::clean_environment(&mut c);
            c.env("AGENT_MAIL_BASH", root.join("absent"))
                .arg("--agmsg-dir")
                .arg(root)
                .current_dir(root)
                .args([
                    "join",
                    "dev",
                    "--name",
                    &format!("worker-{n}"),
                    "--type",
                    "codex",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for c in children {
        ok(c.wait_with_output().unwrap());
    }
    assert_eq!(messages(root, &["agmsg", "members", "dev"]).len(), 9);
    let failure = cli(
        root,
        &["join", "dev", "--name", "sender", "--type", "claude-code"],
    );
    assert!(!failure.status.success());
    assert!(String::from_utf8_lossy(&failure.stderr).contains("another registration"));
}
#[test]
fn per_team_mailbox_and_legacy_rows_are_read_without_duplicates() {
    let temp = fixture();
    let root = temp.path();
    let path = root.join("teams/dev/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["drivers"] = serde_json::json!({"partition":"per-team"});
    fs::write(path, config.to_string()).unwrap();
    // Registration initializes the selected partition without shell tools.
    ok(cli(
        root,
        &["join", "dev", "--name", "sender", "--type", "codex"],
    ));
    send(root, "sender", "receiver", "new");
    let db = rusqlite::Connection::open(root.join("db/teams/dev/messages.db")).unwrap();
    db.execute("INSERT INTO messages(team,from_agent,to_agent,body) VALUES('dev','sender','receiver','legacy')",[]).unwrap();
    assert_eq!(messages(root, &["agmsg", "messages", "dev"]).len(), 2);
    assert!(ok(cli(root, &["agmsg", "inbox", "dev", "receiver"])).contains("2 new message(s):"));
    assert_eq!(
        ok(cli(root, &["agmsg", "inbox", "dev", "receiver"])).trim(),
        "No new messages."
    );
    let shared = rusqlite::Connection::open(root.join("db/messages.db")).unwrap();
    assert_eq!(
        shared
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn reading_a_fresh_upstream_team_does_not_create_a_database() {
    let temp = fixture();
    let root = temp.path();
    fs::remove_dir_all(root.join("db")).unwrap();
    assert!(messages(root, &["agmsg", "messages", "dev"]).is_empty());
    assert_eq!(
        ok(cli(root, &["agmsg", "inbox", "dev", "receiver"])).trim(),
        "No new messages."
    );
    assert!(!root.join("db").exists());
}

#[test]
fn malformed_roster_is_reported_without_erasing_registrations() {
    let temp = fixture();
    let root = temp.path();
    let config = root.join("teams/dev/config.json");
    let before = fs::read(&config).unwrap();
    fs::write(root.join("teams/dev/roster.jsonl"), "{}\n").unwrap();
    let output = cli(
        root,
        &["join", "dev", "--name", "another", "--type", "codex"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("roster journal"));
    assert_eq!(fs::read(config).unwrap(), before);
}
