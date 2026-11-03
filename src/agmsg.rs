use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Member {
    pub name: String,
    pub types: Vec<String>,
    pub project: Option<String>,
}

/// agmsg integration: queries use official scripts; send uses the pinned native writer.
pub struct Agmsg {
    root: PathBuf,
    scripts: PathBuf,
    bash: OsString,
}

pub(crate) fn shell_path(path: &Path) -> OsString {
    if cfg!(windows) {
        windows_shell_path(&path.to_string_lossy()).into()
    } else {
        path.as_os_str().to_owned()
    }
}

fn windows_shell_path(path: &str) -> String {
    // canonicalize() returns verbatim paths on Windows; Git Bash expects normal
    // drive/UNC paths, including when the installation contains spaces.
    if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("//{}", unc.replace('\\', "/"))
    } else {
        path.strip_prefix("\\\\?\\")
            .unwrap_or(path)
            .replace('\\', "/")
    }
}

impl Agmsg {
    pub(crate) async fn resume_daemon(&self, operation: u64) -> Result<(), String> {
        let mut command = tokio::process::Command::new(&self.bash);
        command
            .arg(shell_path(&self.scripts.join("daemon.sh")))
            .args(["start", "--only-if-op-gen", &operation.to_string()])
            .kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(45), command.output())
            .await
            .map_err(|_| "agmsgd restart timed out; check agmsg daemon status")?
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "agmsgd could not resume after the bridge update: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
    pub(crate) async fn stop_remote(&self, team: &str) -> Result<(), String> {
        crate::store::team_name(team)?;
        let mut command = tokio::process::Command::new(&self.bash);
        // v1.7.0 has no public `sync stop`. Reuse its authenticated stop
        // implementation under the same team lock, without disconnecting or
        // changing identity. This internal contract is pinned by the bridge.
        command.arg("-c").arg(r#"
source "$1/scripts/remote.sh"
team="$2"
agmsg_lock_acquire "$TEAMS_DIR/$team" || exit 1
IFS=$'\t' read -r state ignored < <(_remote_systemd_engine_status "$team")
case "$state" in
  running|starting|inactive|unknown)
    echo 'an official systemd unit owns this team; disable that user unit before native adoption' >&2
    agmsg_lock_release
    exit 1 ;;
esac
result=0
_remote_sync_engine_stop "$team" || result=$?
agmsg_lock_release
exit "$result"
"#).arg("agent-mail-stop-official").arg(shell_path(&self.root)).arg(team);
        command.kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
            .await
            .map_err(|_| "official sync stop timed out; inspect remote.sh status before retrying")?
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "official sync stop failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    /// Bounded, read-only probe; never starts daemon or registers a session.
    pub async fn version(&self) -> Result<String, String> {
        let mut command = tokio::process::Command::new(&self.bash);
        command
            .arg(shell_path(&self.scripts.join("agmsg")))
            .arg("--version");
        command.kill_on_drop(true);
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), command.output())
            .await
            .map_err(|_| "agmsg version check timed out after 5 seconds".to_string())?
            .map_err(|e| format!("cannot run agmsg via bash: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "agmsg version check failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let version = String::from_utf8(output.stdout)
            .map_err(|e| format!("agmsg version is not UTF-8: {e}"))?;
        if version.trim().is_empty() {
            return Err("agmsg version check returned empty output".into());
        }
        Ok(version.trim().to_owned())
    }

    pub fn new(root: Option<PathBuf>) -> Result<Self, String> {
        let root = match root {
            Some(root) => root,
            None => {
                let home = if cfg!(windows) {
                    env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"))
                } else {
                    env::var_os("HOME")
                };
                PathBuf::from(home.ok_or(
                    "user home is unavailable; specify --agmsg-dir or AGENT_MAIL_AGMSG_DIR",
                )?)
                .join(".agents/skills/agmsg")
            }
        };
        let root = root.canonicalize().map_err(|error| {
            format!(
                "cannot open agmsg installation {}: {error}; install agmsg separately or specify --agmsg-dir",
                root.display()
            )
        })?;
        let scripts = root.join("scripts");
        if !scripts.join("agmsg").is_file() || !scripts.join("api.sh").is_file() {
            return Err(format!(
                "{} does not contain agmsg's scripts/agmsg and scripts/api.sh",
                root.display()
            ));
        }
        let bash = env::var_os("AGENT_MAIL_BASH").unwrap_or_else(|| OsString::from("bash"));
        Ok(Self {
            root,
            scripts,
            bash,
        })
    }

    pub fn send(
        &self,
        team: &str,
        from: &str,
        to: &str,
        body_file: Option<&Path>,
    ) -> Result<(), String> {
        crate::native_send::send(&self.root, team, from, to, body_file)?;
        println!("Sent to {to} in team {team}");
        Ok(())
    }

    pub fn members(&self, team: &str) -> Result<Vec<Member>, String> {
        let path = self.scripts.join("api.sh");
        let output = Command::new(&self.bash)
            .arg(shell_path(&path))
            .args(["get", "teams", team, "members"])
            .output()
            .map_err(|error| format!("failed to run {} via bash: {error}", path.display()))?;
        // Preserve upstream diagnostics even when stdout must be captured for JSONL parsing.
        use std::io::Write;
        std::io::stderr()
            .write_all(&output.stderr)
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!("agmsg member query failed: {}", output.status));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|e| format!("agmsg member output is not UTF-8: {e}"))?;
        parse_members(&text)
    }

    pub fn run<S: AsRef<OsStr>>(&self, script: &str, args: &[S]) -> Result<ExitStatus, String> {
        let path = self.scripts.join(script);
        // Pass argv directly, without `sh -c` or interpolating user input into shell code.
        Command::new(&self.bash)
            .arg(shell_path(&path))
            .args(args)
            .status()
            .map_err(|error| format!("failed to run {} via bash: {error}", path.display()))
    }
}

fn parse_members(text: &str) -> Result<Vec<Member>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line)
                .map_err(|e| format!("invalid agmsg member JSON at line {}: {e}", index + 1))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_bash_paths_accept_windows_canonical_drive_and_unc_names() {
        assert_eq!(
            windows_shell_path(r"\\?\C:\Users\Test User\邮件\reply.txt"),
            "C:/Users/Test User/邮件/reply.txt"
        );
        assert_eq!(
            windows_shell_path(r"\\?\UNC\host\share\reply.txt"),
            "//host/share/reply.txt"
        );
        assert_eq!(
            windows_shell_path(r"C:\tools\agmsg\scripts\api.sh"),
            "C:/tools/agmsg/scripts/api.sh"
        );
    }

    #[test]
    fn member_query_accepts_new_fields_but_rejects_partial_or_invalid_output() {
        let text =
            "{\"name\":\"alice\",\"types\":[\"codex\"],\"project\":null,\"future_field\":true}\n";
        let members = parse_members(text).unwrap();
        assert_eq!(members[0].name, "alice");
        assert_eq!(members[0].types, ["codex"]);
        assert!(members[0].project.is_none());
        assert!(
            parse_members(&format!("{text}not JSON"))
                .unwrap_err()
                .contains("line 2")
        );
        assert!(parse_members("{\"name\":\"alice\"}").is_err());
    }
}
