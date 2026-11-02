use std::process::Command;

use serde_json::Value;

#[test]
fn doctor_reports_both_configuration_errors_and_returns_failure() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-mail"))
        .args(["--server", "invalid-url", "--agmsg-dir"])
        .arg(temp.path().join("missing"))
        .arg("doctor")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let checks: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0]["component"], "agmsg");
    assert_eq!(checks[0]["ok"], false);
    assert!(
        checks[0]["detail"]
            .as_str()
            .unwrap()
            .contains("cannot open agmsg installation")
    );
    assert_eq!(checks[1]["component"], "server");
    assert_eq!(checks[1]["ok"], false);
    assert!(
        checks[1]["detail"]
            .as_str()
            .unwrap()
            .contains("invalid --server URL")
    );
}
