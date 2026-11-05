use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::Builder::new()
        .prefix("setup with spaces ")
        .tempdir()
        .unwrap();
    fs::write(temp.path().join("VERSION"), "v1.7.0\n").unwrap();
    let output = run(
        temp.path(),
        &[
            "join", "dev", "--create", "--name", "worker", "--type", "codex",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    temp
}

fn run(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-mail"))
        .env("AGENT_MAIL_BASH", root.join("bash-must-not-run"))
        .env_remove("AGENT_MAIL_SERVER")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .current_dir(root)
        .arg("--agmsg-dir")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn local_team_import_refuses_to_replace_existing_state() {
    let temp = fixture();
    let output = run(temp.path(), &["team", "import", "dev"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists locally"));
}

#[test]
fn joining_unknown_team_requires_explicit_creation() {
    let temp = fixture();
    let output = run(
        temp.path(),
        &["join", "missing", "--name", "worker", "--type", "codex"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--create"));
}

#[test]
fn repeated_join_does_not_run_registration_again() {
    let temp = fixture();
    // No join.sh is installed, so success proves the existing registration was reused.
    let output = run(
        temp.path(),
        &["join", "dev", "--name", "worker", "--type", "codex"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bind_requires_session_evidence() {
    let temp = fixture();
    let output = run(
        temp.path(),
        &["bind", "dev", "--name", "worker", "--type", "codex"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("current session is unknown"));
}
