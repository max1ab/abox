use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn export(directory: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-mail"))
        // Export must work without a running service or an agmsg installation.
        .args(["--server", "invalid-url", "--agmsg-dir", "/missing-agmsg"])
        .args(["skill", "export"])
        .arg(directory)
        .output()
        .unwrap()
}

#[test]
fn exports_bundled_skill_into_a_new_directory_without_runtime_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("skills with spaces/agent-mail");
    let output = export(&destination);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(destination.join("SKILL.md")).unwrap(),
        include_str!("../skills/agent-mail/SKILL.md")
    );
}

#[test]
fn refuses_to_overwrite_an_existing_skill_and_keeps_other_files() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("SKILL.md"), "user customization").unwrap();
    fs::write(temp.path().join("other.txt"), "keep").unwrap();
    let output = export(temp.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("never overwritten"));
    assert_eq!(
        fs::read_to_string(temp.path().join("SKILL.md")).unwrap(),
        "user customization"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("other.txt")).unwrap(),
        "keep"
    );
}
