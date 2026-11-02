use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

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
