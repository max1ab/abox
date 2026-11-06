use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct Member {
    pub name: String,
    pub types: Vec<String>,
    pub project: Option<String>,
}

/// Local mailbox data root; no upstream executable or scripts are required.
pub struct Agmsg {
    root: PathBuf,
}

impl Agmsg {
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
                    return Err("data directory is not empty and has no recognized mailbox format; choose a new --data-dir".into());
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
        if let Some(root) = std::env::var_os("AGENT_MAIL_AGMSG_DIR").filter(|v| !v.is_empty()) {
            return Ok(crate::store::native_path(Path::new(&root)));
        }
        let home = crate::store::home()?;
        let old = home.join(".agents/skills/agmsg");
        if old.join("VERSION").is_file() {
            Ok(old)
        } else {
            Ok(home.join(".agent-mail"))
        }
    }

    pub fn new(root: Option<PathBuf>) -> Result<Self, String> {
        let root = Self::location(root)?;
        let root = root.canonicalize().map_err(|error| {
            format!(
                "cannot open Agent Mail data directory {}: {error}; run team import or start --create first, or specify --data-dir",
                root.display()
            )
        })?;
        Ok(Self { root })
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
