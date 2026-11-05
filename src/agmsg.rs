use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct Member {
    pub name: String,
    pub types: Vec<String>,
    pub project: Option<String>,
}

/// Local mailbox root plus the remaining upstream setup/notification bridge.
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
    /// Read-only local format check; never starts Bash or a notification service.
    pub fn version(&self) -> Result<String, String> {
        let version = std::fs::read_to_string(self.root.join("VERSION"))
            .map_err(|e| format!("cannot read mailbox format: {e}"))?;
        if version.trim().trim_start_matches('v') != "1.7.0" {
            return Err(
                "unsupported local mailbox format; expected agmsg v1.7.0 compatible data".into(),
            );
        }
        Ok("Agent Mail native mailbox (agmsg v1.7.0 format)".into())
    }

    pub fn for_setup(root: Option<PathBuf>) -> Result<Self, String> {
        use std::{fs, io::Write};
        let root = Self::location(root)?;
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let init = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".agent-mail-init.lock"))
            .map_err(|e| e.to_string())?;
        init.lock().map_err(|e| e.to_string())?;
        let marker = root.join("VERSION");
        if !marker.try_exists().map_err(|e| e.to_string())? {
            for entry in fs::read_dir(&root).map_err(|e| e.to_string())? {
                if entry.map_err(|e| e.to_string())?.file_name() != ".agent-mail-init.lock" {
                    return Err("data directory is not empty and has no recognized mailbox format; choose a new --agmsg-dir".into());
                }
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(marker)
                .map_err(|e| e.to_string())?;
            file.write_all(b"1.7.0\n")
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
        }
        let client = Self::new(Some(root))?;
        client.version()?;
        Ok(client)
    }

    fn location(root: Option<PathBuf>) -> Result<PathBuf, String> {
        if let Some(root) = root {
            return Ok(root);
        }
        Ok(crate::store::home()?.join(".agents/skills/agmsg"))
    }

    pub fn new(root: Option<PathBuf>) -> Result<Self, String> {
        let root = Self::location(root)?;
        let root = root.canonicalize().map_err(|error| {
            format!(
                "cannot open Agent Mail data directory {}: {error}; run team import or join --create first, or specify --agmsg-dir",
                root.display()
            )
        })?;
        let scripts = root.join("scripts");
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
        crate::mailbox::members(&self.root, team)
    }
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
}
