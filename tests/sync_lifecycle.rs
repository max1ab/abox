mod common;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-mail"));
    common::clean_environment(&mut command);
    let output = command
        .arg("--agmsg-dir")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    if !output.status.success()
        && let Ok(log) = fs::read_to_string(root.join("run/native-sync/contract/engine.log"))
    {
        eprintln!("isolated engine log: {log}");
    }
    output
}
fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn shell(root: &Path, program: &str) -> Output {
    let mut command = Command::new("bash");
    common::clean_environment(&mut command);
    command
        .args(["-c", program, "lifecycle-test"])
        .arg(root)
        .output()
        .unwrap()
}
struct Cleanup<'a>(&'a Path);
impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = cli(self.0, &["sync", "disable", "contract"]);
        if self.0.join("run/install.db").exists() {
            let mut command = Command::new("bash");
            common::clean_environment(&mut command);
            let _ = command
                .arg(self.0.join("scripts/daemon.sh"))
                .arg("stop")
                .output();
        }
    }
}

#[test]
#[ignore = "requires AGENT_MAIL_TEST_AGMSG_DIR, Bash, sqlite3 and Node >=22.13; uses an isolated official agmsgd"]
fn publishing_the_bridge_resumes_an_enabled_official_notification_daemon() {
    let temp = common::fixture();
    let root = temp.path();
    let _cleanup = Cleanup(root);
    let path = root.join("teams/contract/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["remote_binding"] = json!({"endpoint":"http://127.0.0.1:9", "server_instance_id":uuid::Uuid::now_v7().to_string(), "remote_team_id":uuid::Uuid::now_v7().to_string(), "protocol_version":1, "cipher_profile":"none", "disconnected_at":null});
    fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::create_dir_all(root.join("run")).unwrap();
    ok(shell(
        root,
        r#"source "$1/scripts/lib/install-manifest.sh"; agmsg_install_manifest_write "$1/scripts" "$1/run/install-manifest.json" v1.7.0 lifecycle-daemon-test 1 1"#,
    ));
    let db = rusqlite::Connection::open(root.join("run/install.db")).unwrap();
    db.execute_batch(&fs::read_to_string(root.join("scripts/daemon/schema.sql")).unwrap())
        .unwrap();
    db.execute("UPDATE meta SET install_id='lifecycle-daemon-test'", [])
        .unwrap();
    // No enable command: starting an unregistered daemon does not install a
    // user service. Its install-root-derived unit cannot name a real daemon.
    ok(shell(root, r#"bash "$1/scripts/daemon.sh" start"#));
    let before: u64 = db
        .query_row(
            "SELECT gen FROM daemon_owner WHERE state='ready'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    ok(cli(root, &["sync", "enable", "contract", "--no-login"]));
    let after: u64 = db
        .query_row(
            "SELECT gen FROM daemon_owner WHERE state='ready'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        after > before,
        "the previous generation must not silently remain as ready"
    );
    verify(root);
}
fn manifest(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("run/install-manifest.json")).unwrap()).unwrap()
}
fn verify(root: &Path) {
    let m = manifest(root);
    for entry in m["files"].as_array().unwrap() {
        let bytes = fs::read(root.join(entry["path"].as_str().unwrap())).unwrap();
        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(entry["digest"], hex);
    }
}

#[test]
#[ignore = "requires AGENT_MAIL_TEST_AGMSG_DIR, Bash and sqlite3; only temporary installations are modified"]
fn native_hooks_restart_the_selected_engine_and_preserve_upstream_integrity() {
    let temp = common::fixture();
    let root = temp.path();
    let _cleanup = Cleanup(root);
    let path = root.join("teams/contract/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["remote_binding"] = json!({
        "endpoint":"http://127.0.0.1:9", "server_instance_id":uuid::Uuid::now_v7().to_string(),
        "remote_team_id":uuid::Uuid::now_v7().to_string(), "protocol_version":1,
        "cipher_profile":"none", "disconnected_at":null,
    });
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::create_dir_all(root.join("run")).unwrap();
    ok(shell(
        root,
        r#"source "$1/scripts/lib/install-manifest.sh"; agmsg_install_manifest_write "$1/scripts" "$1/run/install-manifest.json" v1.7.0 lifecycle-test 1 1"#,
    ));
    let original = manifest(root);
    // Rejection is before source, manifest or engine selection changes.
    config["remote_binding"]["cipher_profile"] = json!("age-v1");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(
        !cli(root, &["sync", "enable", "contract", "--no-login"])
            .status
            .success()
    );
    assert_eq!(manifest(root), original);
    assert!(!root.join("run/native-sync/contract/engine").exists());
    config["remote_binding"]["cipher_profile"] = json!("none");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    // Windows npm installs ship the release number without the optional v.
    fs::write(root.join("VERSION"), "1.7.0\n").unwrap();

    let selected: Value = serde_json::from_str(&ok(cli(
        root,
        &["sync", "enable", "contract", "--no-login"],
    )))
    .unwrap();
    assert_eq!(selected["running"], true); // Readiness is IPC, even with server offline.
    let updated = manifest(root);
    assert_eq!(updated["gen"], 2);
    assert_eq!(updated["install_id"], original["install_id"]);
    let changed: Vec<_> = original["files"]
        .as_array()
        .unwrap()
        .iter()
        .zip(updated["files"].as_array().unwrap())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0["path"], "scripts/remote.sh");
    verify(root);
    let endpoint = root.join("run/native-sync/contract/endpoint.json");
    let first = fs::read(&endpoint).unwrap();
    ok(cli(root, &["sync", "start", "contract"]));
    assert_eq!(fs::read(&endpoint).unwrap(), first);
    ok(cli(root, &["sync", "enable", "contract", "--no-login"]));
    assert_eq!(manifest(root)["gen"], 2); // No repeated manifest generation churn.
    ok(cli(root, &["sync", "stop", "contract"]));
    assert_eq!(
        serde_json::from_str::<Value>(&ok(cli(root, &["sync", "status", "contract"]))).unwrap()["running"],
        false
    );
    ok(shell(
        root,
        r#"source "$1/scripts/lib/sync-autostart.sh"; agmsg_sync_autostart "$1/scripts/remote.sh" contract"#,
    ));
    assert_eq!(
        serde_json::from_str::<Value>(&ok(cli(root, &["sync", "status", "contract"]))).unwrap()["running"],
        true
    );
    assert!(!root.join("run/remote-sync.contract.pid").exists());
    ok(cli(root, &["sync", "disable", "contract"]));
    assert!(!root.join("run/native-sync/contract/engine").exists());
    verify(root);
    // Unselected teams must still call upstream; do not start a real Node
    // transport for this assertion.
    let fallback = tempfile::tempdir().unwrap();
    fs::create_dir_all(fallback.path().join("scripts")).unwrap();
    let remote = fallback.path().join("scripts/remote.sh");
    fs::write(&remote, "#!/bin/bash\nprintf '%s\\n' \"$*\" > \"${0%/scripts/remote.sh}/called\"\necho 'already running'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&remote, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut command = Command::new("bash");
    common::clean_environment(&mut command);
    ok(command.args(["-c", r#"source "$1/scripts/lib/sync-autostart.sh"; agmsg_sync_autostart "$2/scripts/remote.sh" other"#, "fallback-test"]).arg(root).arg(fallback.path()).output().unwrap());
    assert_eq!(
        fs::read_to_string(fallback.path().join("called")).unwrap(),
        "sync start other\n"
    );

    // Simulate a crash after publishing the patched helper but before manifest.
    fs::remove_file(root.join("run/install-manifest.json")).unwrap();
    ok(cli(root, &["sync", "enable", "contract", "--no-login"]));
    assert_eq!(manifest(root)["gen"], 2);
    verify(root);
    ok(cli(root, &["sync", "disable", "contract"]));
    let current = fs::read(root.join("run/install-manifest.json")).unwrap();
    fs::write(root.join("scripts/api.sh"), "unexpected modification").unwrap();
    assert!(
        !cli(root, &["sync", "enable", "contract", "--no-login"])
            .status
            .success()
    );
    assert_eq!(
        fs::read(root.join("run/install-manifest.json")).unwrap(),
        current
    );
    assert!(!root.join("run/native-sync/contract/engine").exists());
}
