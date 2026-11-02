use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

fn clean_environment(command: &mut Command) {
    for name in [
        "AGMSG_STORAGE_PATH",
        "AGMSG_PLUGIN_DIRS",
        "SKILL_DIR",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
    ] {
        command.env_remove(name);
    }
    command.env("AGMSG_RESOLVE_PROJECT", "0");
    command.env("AGMSG_SELF_NAME", "off");
    command.env("AGMSG_SELF_RENAME", "off");
}

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

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

// Copy only upstream code. Every test owns fresh teams, storage and runtime state;
// the source installation and its real mailboxes are never modified.
fn fixture() -> TempDir {
    let source = std::env::var_os("AGENT_MAIL_TEST_AGMSG_DIR")
        .expect("set AGENT_MAIL_TEST_AGMSG_DIR to an agmsg checkout or installation root");
    let temp = tempfile::Builder::new()
        .prefix("agent mail test ")
        .tempdir()
        .unwrap();
    copy_tree(
        &Path::new(&source).join("scripts"),
        &temp.path().join("scripts"),
    );
    for agent in ["sender", "receiver"] {
        let mut command = Command::new("bash");
        clean_environment(&mut command);
        let output = command
            .arg(temp.path().join("scripts/join.sh"))
            .args(["contract", agent, "codex"])
            .arg(temp.path())
            .output()
            .unwrap();
        success(output);
    }
    temp
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
