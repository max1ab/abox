use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
mod common;
use common::{clean_environment, fixture};

fn cli(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    clean_environment(&mut command);
    command.arg("--agmsg-dir").arg(root).arg("agmsg");
    command
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn history(root: &Path) -> Vec<Value> {
    success(cli(root).args(["messages", "contract"]).output().unwrap())
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn send_requires_exactly_one_body_source() {
    // Clap rejects these before loading any agmsg installation.
    for args in [
        vec!["send", "contract", "--from", "sender", "--to", "receiver"],
        vec![
            "send",
            "contract",
            "--from",
            "sender",
            "--to",
            "receiver",
            "--stdin",
            "--body-file",
            "unused.txt",
        ],
    ] {
        let output = cli(Path::new("/nonexistent-agent-mail-test"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR; uses isolated upstream scripts"]
fn setup_registers_and_verifies_codex_and_claude_sessions() {
    let temp = fixture();
    let root = temp.path();
    let invoke = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
        clean_environment(&mut command);
        command
            .arg("--agmsg-dir")
            .arg(root)
            .env("CODEX_HOME", root.join("codex-profile"))
            .env("CLAUDE_CONFIG_DIR", root.join("claude-profile"))
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CLAUDE_PID")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap()
    };
    success(invoke(&[
        "join", "new-team", "--create", "--name", "tester", "--type", "codex",
    ]));
    success(invoke(&[
        "join", "new-team", "--name", "tester", "--type", "codex",
    ]));
    let output = success(invoke(&[
        "bind",
        "new-team",
        "--name",
        "tester",
        "--type",
        "codex",
        "--session-id",
        "01900000-0000-7000-8000-000000000001",
    ]));
    assert!(output.contains("Bound new-team/tester"));
    success(invoke(&[
        "join",
        "new-team",
        "--name",
        "claude-tester",
        "--type",
        "claude-code",
    ]));
    let output = success(invoke(&[
        "bind",
        "new-team",
        "--name",
        "claude-tester",
        "--type",
        "claude-code",
        "--session-id",
        "01900000-0000-7000-8000-000000000002",
    ]));
    assert!(output.contains("Bound new-team/claude-tester"));
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR; uses real upstream scripts"]
fn file_message_survives_history_and_is_read_only_once() {
    let temp = fixture();
    let root = temp.path();
    let marker = root.join("must-not-exist");
    let body = format!(
        "中文正文\nquotes: 'single' \"double\"\n$(touch '{}')\n`echo literal`\n{}\n\n",
        marker.display(),
        "message content ".repeat(128)
    );
    let file = root.join("body 'with spaces'.txt");
    fs::write(&file, &body).unwrap();
    success(
        cli(root)
            .args([
                "send",
                "contract",
                "--from",
                "sender",
                "--to",
                "receiver",
                "--body-file",
            ])
            .arg(&file)
            .output()
            .unwrap(),
    );
    assert!(
        !marker.exists(),
        "message content must never become shell code"
    );
    let rows = history(root);
    assert_eq!(
        rows.len(),
        1,
        "history must not duplicate legacy/event copies"
    );
    assert_eq!(rows[0]["body"], body.trim_end_matches('\n'));
    assert_eq!(rows[0]["from"], "sender");
    assert_eq!(rows[0]["to"], "receiver");

    let first = success(
        cli(root)
            .args(["inbox", "contract", "receiver"])
            .output()
            .unwrap(),
    );
    assert!(
        first.contains("1 new message(s):"),
        "history must leave mail unread"
    );
    assert!(first.contains("中文正文"));
    let second = success(
        cli(root)
            .args(["inbox", "contract", "receiver"])
            .output()
            .unwrap(),
    );
    assert_eq!(second.trim(), "No new messages.");
    assert_eq!(
        history(root),
        rows,
        "reading an inbox must preserve message history"
    );
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR; uses real upstream scripts"]
fn piped_message_preserves_content_and_rejects_empty_body() {
    let temp = fixture();
    let root = temp.path();
    for (body, expected_code) in [("line one\n第二行\n", 0), ("", 1)] {
        let mut child = cli(root)
            .args([
                "send", "contract", "--from", "sender", "--to", "receiver", "--stdin",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(expected_code));
        if expected_code != 0 {
            assert!(String::from_utf8_lossy(&output.stderr).contains("empty"));
        }
    }
    let rows = history(root);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["body"], "line one\n第二行");
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR; uses real upstream scripts"]
fn unregistered_recipient_preserves_upstream_exit_and_stderr() {
    let temp = fixture();
    let root = temp.path();
    let file = root.join("body.txt");
    fs::write(&file, "must not be stored").unwrap();
    let output = cli(root)
        .args([
            "send",
            "contract",
            "--from",
            "sender",
            "--to",
            "unknown",
            "--body-file",
        ])
        .arg(file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not registered"));
    assert!(history(root).is_empty());
}

fn native_send(root: &Path, file: &Path) -> Command {
    let mut cmd = cli(root);
    cmd.env("AGENT_MAIL_BASH", root.join("bash-must-not-run"));
    cmd.args([
        "send",
        "contract",
        "--from",
        "sender",
        "--to",
        "receiver",
        "--body-file",
    ]);
    cmd.arg(file);
    cmd
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR for isolated official fixture setup/readback"]
fn native_send_uses_no_shell_and_rolls_back_both_rows_on_failure() {
    let temp = fixture();
    let root = temp.path();
    let file = root.join("body.txt");
    fs::write(&file, "native rollback test").unwrap();
    let db = rusqlite::Connection::open(root.join("db/messages.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    let out = native_send(root, &file).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("rolled back"));
    for table in ["messages", "events"] {
        let count: i64 = db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    db.execute_batch("DROP TRIGGER reject_event;").unwrap();
    success(native_send(root, &file).output().unwrap());
    assert_eq!(history(root).len(), 1);
    let id: String = db
        .query_row("SELECT id FROM events WHERE type='message_sent'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(uuid::Uuid::parse_str(&id).unwrap().get_version_num(), 7);
    let inbox = success(
        cli(root)
            .args(["inbox", "contract", "receiver"])
            .output()
            .unwrap(),
    );
    assert!(inbox.contains("1 new message(s):"));
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR for isolated official fixture setup/readback"]
fn native_send_resolves_per_team_store_and_serializes_writers() {
    let temp = fixture();
    let root = temp.path();
    let cfg_path = root.join("teams/contract/config.json");
    let mut cfg: Value = serde_json::from_slice(&fs::read(&cfg_path).unwrap()).unwrap();
    cfg["drivers"] = serde_json::json!({"partition":"per-team"});
    fs::write(cfg_path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    let dest = root.join("db/teams/contract");
    fs::create_dir_all(&dest).unwrap();
    // SQLite backup includes the initialized schema without depending on WAL checkpoint timing.
    let db = rusqlite::Connection::open(root.join("db/messages.db")).unwrap();
    db.execute(
        "VACUUM INTO ?1",
        [dest.join("messages.db").to_str().unwrap()],
    )
    .unwrap();
    let copied = rusqlite::Connection::open(dest.join("messages.db")).unwrap();
    copied.pragma_update(None, "journal_mode", "WAL").unwrap();
    drop(copied);
    let file = root.join("body.txt");
    fs::write(&file, "parallel native send").unwrap();
    let children: Vec<_> = (0..8)
        .map(|_| {
            native_send(root, &file)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        success(child.wait_with_output().unwrap());
    }
    assert_eq!(history(root).len(), 8);
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "per-team sends must not leak into the shared store"
    );
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR for isolated official fixture setup/readback"]
fn native_send_refuses_unknown_storage_versions_and_path_traversal() {
    let temp = fixture();
    let root = temp.path();
    let file = root.join("body.txt");
    fs::write(&file, "must not be written").unwrap();
    let out = native_send(root, &file)
        .env("AGMSG_STORAGE_DRIVER", "jsonl")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not support storage"));
    fs::write(root.join("VERSION"), "v99.0.0").unwrap();
    let out = native_send(root, &file).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("v1.7.0 only"));
    fs::write(root.join("VERSION"), "v1.7.0").unwrap();
    let out = cli(root)
        .args([
            "send",
            "../escape",
            "--from",
            "sender",
            "--to",
            "receiver",
            "--body-file",
        ])
        .arg(file)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid team name"));
    assert!(history(root).is_empty());
}

#[test]
#[ignore = "requires Bash, sqlite3, jq, Node and AGENT_MAIL_TEST_AGMSG_DIR; tests official sync without networking"]
fn official_sync_prepares_native_message_once_and_preserves_body() {
    let temp = fixture();
    let root = temp.path();
    let body = "中文 remote body with 'quotes' and \\backslash\nsecond line\r\n";
    let file = root.join("body.txt");
    fs::write(&file, body).unwrap();
    success(native_send(root, &file).output().unwrap());
    let prepare = || {
        let mut cmd = Command::new("bash");
        clean_environment(&mut cmd);
        success(cmd.arg("-c").arg(r#"
set -euo pipefail
export SKILL_DIR="$1"
source "$1/scripts/lib/storage.sh"
agmsg_storage_load
printf '%s\n' '{"type":"sync_prepare","envelope_v":1,"cipher":"none","key_id":null,"max_blob_bytes":1048576,"allow_new":true}' | storage_sync_prepare_push contract 018f3f7e-0000-7000-8000-000000000000 018f3f7e-0000-7000-8000-000000000001 1 100
"#).arg("sync-fixture").arg(root).output().unwrap())
    };
    let first = prepare();
    assert_eq!(first, prepare(), "retry must reuse the same wire envelope");
    let candidates: Vec<Value> = first
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|v| v["type"] == "sync_push_candidate")
        .collect();
    assert_eq!(
        candidates.len(),
        1,
        "legacy/event mirror must produce one upload"
    );
    let blob = candidates[0]["envelope"]["blob"].as_str().unwrap();
    let output = Command::new("node")
        .args([
            "-e",
            "process.stdout.write(Buffer.from(process.argv[1], 'base64').toString('utf8'))",
            blob,
        ])
        .output()
        .unwrap();
    let projection: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(projection["body"], body.trim_end_matches('\n'));
    assert_eq!(projection["from_agent"], "sender");
    assert_eq!(projection["to_agent"], "receiver");
}

#[test]
#[ignore = "requires Bash, sqlite3 and AGENT_MAIL_TEST_AGMSG_DIR; uses isolated official scripts"]
fn native_and_official_inboxes_share_the_same_read_state() {
    let temp = fixture();
    let root = temp.path();
    let file = root.join("body.txt");
    fs::write(&file, "native to official").unwrap();
    success(native_send(root, &file).output().unwrap());
    let upstream = |script: &str, args: &[&str]| {
        let mut command = Command::new("bash");
        clean_environment(&mut command);
        success(
            command
                .arg(root.join("scripts").join(script))
                .args(args)
                .output()
                .unwrap(),
        )
    };
    assert!(upstream("inbox.sh", &["contract", "receiver"]).contains("1 new message(s):"));
    assert_eq!(
        success(
            cli(root)
                .args(["inbox", "contract", "receiver"])
                .output()
                .unwrap()
        )
        .trim(),
        "No new messages."
    );
    fs::write(&file, "official to native").unwrap();
    upstream(
        "send.sh",
        &["contract", "sender", "receiver", file.to_str().unwrap()],
    );
    assert!(
        success(
            cli(root)
                .args(["inbox", "contract", "receiver"])
                .output()
                .unwrap()
        )
        .contains("1 new message(s):")
    );
    assert_eq!(
        upstream("inbox.sh", &["contract", "receiver"]).trim(),
        "No new messages."
    );
}
