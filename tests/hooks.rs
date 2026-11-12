use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
#[allow(dead_code)]
mod common;
const FIRST: &str = "019a4000-0000-7000-8000-000000000001";
const SECOND: &str = "019a4000-0000-7000-8000-000000000002";
fn command(root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_abox"));
    common::clean_environment(&mut c);
    c.env("AGENT_MAIL_BASH", root.join("absent-bash"))
        .env("CODEX_HOME", root)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CLAUDE_CODE_MESSAGING_SOCKET")
        .env_remove("CLAUDE_CODE_MESSAGING_TOKEN")
        .arg("--data-dir")
        .arg(root)
        .current_dir(root)
        .args(args);
    c
}
fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn fixture(kind: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    fs::write(r.join("VERSION"), "1.7.0\n").unwrap();
    for name in ["one", "two"] {
        ok(command(
            r,
            &["join", "dev", "--create", "--name", name, "--type", kind],
        )
        .output()
        .unwrap());
    }
    t
}
fn input(mut c: Command, value: Value) -> Output {
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(value.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
#[test]
fn installing_and_uninstalling_hooks_preserves_unrelated_configuration() {
    let t = fixture("codex");
    let r = t.path();
    let path = r.join("profile/hooks.json");
    fs::create_dir(path.parent().unwrap()).unwrap();
    let original = json!({"permissions":{"deny":["keep-policy"]},"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"keep-command"}]}]}});
    fs::write(&path, original.to_string()).unwrap();
    for _ in 0..2 {
        ok(command(
            r,
            &[
                "hook",
                "install",
                "--type",
                "codex",
                "--config",
                path.to_str().unwrap(),
            ],
        )
        .output()
        .unwrap());
    }
    let cfg: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(cfg["permissions"], original["permissions"]);
    assert_eq!(cfg["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    let native = &cfg["hooks"]["SessionStart"][1]["hooks"][0];
    assert!(native["command"].as_str().unwrap().contains("hook"));
    assert!(native["commandWindows"].as_str().unwrap().starts_with("& "));
    assert!(!native.to_string().contains("bash"));
    ok(command(
        r,
        &[
            "hook",
            "uninstall",
            "--type",
            "codex",
            "--config",
            path.to_str().unwrap(),
        ],
    )
    .output()
    .unwrap());
    let cfg: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(cfg, original);
}
#[test]
fn legacy_hooks_are_replaced_only_when_explicitly_requested() {
    let t = fixture("codex");
    let r = t.path();
    let path = r.join("hooks.json");
    let command_text = format!("bash '{}/scripts/session-start.sh' codex", r.display());
    fs::write(&path,json!({"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":command_text},{"type":"command","command":"keep"}]}]}}).to_string()).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        !command(
            r,
            &[
                "hook",
                "install",
                "--type",
                "codex",
                "--config",
                path.to_str().unwrap()
            ]
        )
        .output()
        .unwrap()
        .status
        .success()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    ok(command(
        r,
        &[
            "hook",
            "install",
            "--type",
            "codex",
            "--config",
            path.to_str().unwrap(),
            "--replace-agmsg",
        ],
    )
    .output()
    .unwrap());
    let text = fs::read_to_string(path).unwrap();
    assert!(!text.contains("session-start.sh"));
    assert!(text.contains("keep"));
}
#[test]
fn hook_refreshes_only_the_matching_session_and_supports_subdirectory_cwd() {
    let t = fixture("codex");
    let r = t.path();
    for (name, id) in [("one", FIRST), ("two", SECOND)] {
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
                id,
            ],
        )
        .output()
        .unwrap());
    }
    let second = fs::read(r.join("run/role-session.dev__two")).unwrap();
    let sub = r.join("subdirectory");
    fs::create_dir(&sub).unwrap();
    let profile = r.join("new profile");
    let mut hook = command(r, &["hook", "run", "--type", "codex"]);
    hook.env("CODEX_HOME", &profile);
    let output = input(
        hook,
        json!({"session_id":FIRST,"hook_event_name":"SessionStart","cwd":sub}),
    );
    let value: Value = serde_json::from_str(ok(output).trim()).unwrap();
    assert!(
        value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("dev/one")
    );
    assert_eq!(
        fs::read(r.join("run/role-session.dev__two")).unwrap(),
        second
    );
    assert!(
        fs::read_to_string(r.join("run/role-session.dev__one"))
            .unwrap()
            .contains(&format!("codex_home={}\n", profile.display()))
    );
}
#[test]
fn claude_clear_refreshes_the_uuid_only_when_the_socket_matches() {
    let t = fixture("claude-code");
    let r = t.path();
    let socket = r.join("cc-socket");
    ok(command(
        r,
        &["bind", "dev", "--name", "one", "--type", "claude-code"],
    )
    .env("CLAUDE_CODE_SESSION_ID", FIRST)
    .env("CLAUDE_CONFIG_DIR", r)
    .env("CLAUDE_CODE_MESSAGING_SOCKET", &socket)
    .output()
    .unwrap());
    let mut c = command(r, &["hook", "run", "--type", "claude-code"]);
    c.env("CLAUDE_CODE_SESSION_ID", SECOND)
        .env("CLAUDE_CONFIG_DIR", r)
        .env("CLAUDE_CODE_MESSAGING_SOCKET", &socket)
        .env("CLAUDE_CODE_MESSAGING_TOKEN", "new-token");
    ok(input(
        c,
        json!({"session_id":SECOND,"hook_event_name":"SessionStart","cwd":r}),
    ));
    let record = fs::read_to_string(r.join("run/role-session.dev__one")).unwrap();
    assert!(record.contains(&format!("session={SECOND}\n")));
    assert!(record.contains("messaging_token=new-token"));
    assert!(!r.join("run/role-session.dev__two").exists());
}

#[test]
fn inline_legacy_hooks_are_reported_without_modifying_trust_or_other_settings() {
    let t = fixture("codex");
    let r = t.path();
    let path = r.join("hooks.json");
    let original = format!(
        "model = 'keep-model'\n[[hooks.SessionStart]]\n[[hooks.SessionStart.hooks]]\ntype='command'\ncommand=\"bash '{}/scripts/session-start.sh' codex\"\n",
        r.display()
    );
    fs::write(r.join("config.toml"), &original).unwrap();
    let output = command(
        r,
        &[
            "hook",
            "install",
            "--type",
            "codex",
            "--config",
            path.to_str().unwrap(),
            "--replace-agmsg",
        ],
    )
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("config.toml"));
    assert!(!path.exists());
    assert_eq!(fs::read_to_string(r.join("config.toml")).unwrap(), original);
}
