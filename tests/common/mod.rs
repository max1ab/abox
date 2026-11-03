use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

pub fn clean_environment(command: &mut Command) {
    for name in [
        "AGMSG_STORAGE_PATH",
        "AGMSG_STORAGE_DRIVER",
        "AGMSG_CONFIG",
        "AGENT_MAIL_BASH",
        "AGMSG_PLUGIN_DIRS",
        "AGMSG_SYNC_CONNECTION_DIR",
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

// Copy only upstream code, so the source installation's data is never modified.
pub fn fixture() -> TempDir {
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
    fs::copy(
        Path::new(&source).join("VERSION"),
        temp.path().join("VERSION"),
    )
    .unwrap();
    for agent in ["sender", "receiver"] {
        let mut command = Command::new("bash");
        clean_environment(&mut command);
        let output = command
            .arg(temp.path().join("scripts/join.sh"))
            .args(["contract", agent, "codex"])
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "join failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut command = Command::new("bash");
    clean_environment(&mut command);
    let output = command
        .arg("-c")
        .arg("source \"$1/scripts/lib/storage.sh\"; agmsg_storage_load; storage_init contract")
        .arg("fixture-init")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "storage init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    temp
}
