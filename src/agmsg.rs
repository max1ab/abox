use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Member {
    pub name: String,
    pub types: Vec<String>,
    pub project: Option<String>,
}

/// Process boundary to agmsg. Upstream owns its scripts, configuration and storage.
pub struct Agmsg {
    scripts: PathBuf,
}

impl Agmsg {
    pub fn new(root: Option<PathBuf>) -> Result<Self, String> {
        let root = match root {
            Some(root) => root,
            None => PathBuf::from(
                env::var_os("HOME")
                    .ok_or("HOME is unavailable; specify --agmsg-dir or AGENT_MAIL_AGMSG_DIR")?,
            )
            .join(".agents/skills/agmsg"),
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
        Ok(Self { scripts })
    }

    pub fn send(
        &self,
        team: &str,
        from: &str,
        to: &str,
        body_file: Option<&Path>,
    ) -> Result<ExitStatus, String> {
        let (flag, source) = match body_file {
            Some(path) => ("--body-file", path.as_os_str()),
            None => ("--body", OsStr::new("-")),
        };
        self.run(
            "send.sh",
            &[
                OsStr::new(team),
                OsStr::new(from),
                OsStr::new(to),
                OsStr::new(flag),
                source,
            ],
        )
    }

    pub fn members(&self, team: &str) -> Result<Vec<Member>, String> {
        let path = self.scripts.join("api.sh");
        let output = Command::new("bash")
            .arg(&path)
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
        Command::new("bash")
            .arg(&path)
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
