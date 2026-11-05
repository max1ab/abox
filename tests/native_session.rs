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
    for key in [
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_MESSAGING_SOCKET",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
    ] {
        c.env_remove(key);
    }
    c.env("AGENT_MAIL_BASH", root.join("bash-must-not-run"))
        .current_dir(root)
        .arg("--agmsg-dir")
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
fn fixture(kind: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("VERSION"), "1.7.0\n").unwrap();
    for team in ["dev", "other"] {
        ok(command(
            t.path(),
            &["join", team, "--create", "--name", "worker", "--type", kind],
        )
        .output()
        .unwrap());
    }
    t
}
fn args(kind: &str, id: &str) -> Vec<String> {
    vec![
        "bind".into(),
        "dev".into(),
        "--name".into(),
        "worker".into(),
        "--type".into(),
        kind.into(),
        "--session-id".into(),
        id.into(),
    ]
}
fn record(root: &Path) -> String {
    fs::read_to_string(root.join("run/role-session.dev__worker")).unwrap()
}
fn bind(root: &Path, kind: &str, id: &str, replace: bool) -> Output {
    let mut a = args(kind, id);
    if replace {
        a.push("--replace".into());
    }
    command(root, &a.iter().map(String::as_str).collect::<Vec<_>>())
        .output()
        .unwrap()
}
#[test]
fn codex_binding_is_local_to_one_team_and_requires_explicit_replacement() {
    let t = fixture("codex");
    let r = t.path();
    let profile = r.join("codex profile");
    ok(command(
        r,
        &args("codex", FIRST)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )
    .env("CODEX_HOME", &profile)
    .output()
    .unwrap());
    let first = record(r);
    assert!(first.contains(&format!("session={FIRST}\n")));
    assert!(first.contains(&format!("codex_home={}\n", profile.display())));
    assert!(!r.join("run/role-session.other__worker").exists());
    assert!(!r.join("scripts").exists());
    ok(bind(r, "codex", FIRST, false));
    assert!(record(r).contains(&format!("codex_home={}\n", profile.display())));
    let before = record(r);
    let rejected = bind(r, "codex", SECOND, false);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("--replace"));
    assert_eq!(record(r), before);
    ok(bind(r, "codex", SECOND, true));
    assert!(record(r).contains(&format!("session={SECOND}\n")));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(r.join("run/role-session.dev__worker"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn claude_socket_is_attached_only_to_its_runtime_session_and_cleared_on_rebind() {
    let t = fixture("claude-code");
    let r = t.path();
    let socket = r.join("cc-msg.socket");
    let config = r.join("claude profile");
    ok(command(
        r,
        &["bind", "dev", "--name", "worker", "--type", "claude-code"],
    )
    .env("CLAUDE_CODE_SESSION_ID", FIRST)
    .env(
        "CLAUDE_CODE_MESSAGING_SOCKET",
        format!("uds:{}", socket.display()),
    )
    .env("CLAUDE_CONFIG_DIR", &config)
    .env("CLAUDE_CODE_MESSAGING_TOKEN", "test-token")
    .output()
    .unwrap());
    let before = record(r);
    assert!(before.contains(&format!("messaging_socket={}\n", socket.display())));
    assert!(before.contains("messaging_token=test-token\n"));
    assert!(!r.join("run/role-session.other__worker").exists());
    let mismatch = command(
        r,
        &args("claude-code", SECOND)
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )
    .env("CLAUDE_CODE_SESSION_ID", FIRST)
    .output()
    .unwrap();
    assert!(!mismatch.status.success());
    assert_eq!(record(r), before);
    ok(bind(r, "claude-code", FIRST, false));
    assert!(record(r).contains("messaging_token=test-token\n"));
    ok(bind(r, "claude-code", SECOND, true));
    let after = record(r);
    assert!(!after.contains("messaging_socket="));
    assert!(!after.contains("messaging_token="));
    assert!(!after.contains("claude_config_dir="));
}
#[test]
fn binding_refuses_an_existing_foreign_monitor_owner_even_with_replace() {
    let t = fixture("claude-code");
    let r = t.path();
    fs::create_dir_all(r.join("run")).unwrap();
    fs::write(
        r.join("run/actas.dev__worker.session"),
        format!("{FIRST}.1234\n"),
    )
    .unwrap();
    assert!(!bind(r, "claude-code", SECOND, true).status.success());
    assert!(!r.join("run/role-session.dev__worker").exists());
    ok(bind(r, "claude-code", FIRST, false));
}
#[test]
fn binding_checks_registration_and_percent_encodes_unicode_names() {
    let t = fixture("codex");
    let r = t.path();
    let name = "审阅 agent";
    ok(
        command(r, &["join", "dev", "--name", name, "--type", "codex"])
            .output()
            .unwrap(),
    );
    ok(command(
        r,
        &[
            "bind",
            "dev",
            "--name",
            name,
            "--type",
            "codex",
            "--session-id",
            FIRST,
        ],
    )
    .output()
    .unwrap());
    assert!(
        r.join("run/role-session.dev__%E5%AE%A1%E9%98%85%20agent")
            .exists()
    );
    let other = r.join("different-project");
    fs::create_dir(&other).unwrap();
    let output = command(
        r,
        &[
            "bind",
            "dev",
            "--name",
            "worker",
            "--type",
            "codex",
            "--session-id",
            FIRST,
            "--project",
            other.to_str().unwrap(),
        ],
    )
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(!r.join("run/role-session.dev__worker").exists());
}
#[cfg(unix)]
#[test]
fn binding_does_not_follow_a_session_record_symlink() {
    let t = fixture("codex");
    let r = t.path();
    fs::create_dir(r.join("run")).unwrap();
    let victim = r.join("unchanged");
    fs::write(&victim, "keep").unwrap();
    std::os::unix::fs::symlink(&victim, r.join("run/role-session.dev__worker")).unwrap();
    assert!(!bind(r, "codex", FIRST, false).status.success());
    assert_eq!(fs::read_to_string(victim).unwrap(), "keep");
}

#[test]
fn codex_binding_uses_the_profile_previously_published_by_its_session_hook() {
    let t = fixture("codex");
    let r = t.path();
    let profile = r.join("hook profile");
    fs::create_dir(r.join("run")).unwrap();
    fs::write(
        r.join(format!("run/codex-thread-home.{FIRST}")),
        format!("{}\n", profile.display()),
    )
    .unwrap();
    ok(bind(r, "codex", FIRST, false));
    assert!(record(r).contains(&format!("codex_home={}\n", profile.display())));
}
