//! Remove user-login jobs owned by the previous Rust per-team worker.
//! No upstream scripts, Node processes, or foreign jobs are modified.
use super::{Result, runtime};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
fn label(root: &Path, team: &str) -> Result<String> {
    crate::store::team_name(team)?;
    let text = format!(
        "{}\0{team}",
        root.to_str().ok_or("installation path is not UTF-8")?
    );
    let hash = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    Ok(format!(
        "io.agent-mail.sync.{}",
        hash.as_ref()[..12]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}
#[cfg(not(windows))]
fn home() -> Result<PathBuf> {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .ok_or("user home unavailable".into())
}
pub(super) fn configured(root: &Path, team: &str) -> Result<bool> {
    Ok(runtime::directory(root, team)?.join("login").is_file())
}
fn checked(program: &str, args: &[&str]) -> Result<()> {
    let result = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("cannot execute {program}: {e}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed ({}); inspect the user service manager",
            result.status
        ))
    }
}
#[cfg(target_os = "macos")]
fn domain() -> Result<String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| e.to_string())?;
    let uid = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let uid = uid.trim();
    if !output.status.success() || uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) {
        return Err("cannot identify login domain".into());
    }
    Ok(format!("gui/{uid}"))
}
#[cfg(windows)]
fn ps(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}
#[cfg(windows)]
fn powershell(script: &str) -> Result<()> {
    checked(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}
fn job_file(root: &Path, team: &str) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(home()?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", label(root, team)?)))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(home()?
            .join(".config/systemd/user")
            .join(format!("{}.service", label(root, team)?)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(runtime::directory(root, team)?.join("login-job"))
    }
}
pub fn pause(root: &Path, team: &str) -> Result<()> {
    if !configured(root, team)? {
        return Ok(());
    }
    let name = label(root, team)?;
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", domain()?, name);
        checked("launchctl", &["disable", &target])?;
        let _ = checked("launchctl", &["bootout", &target]);
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        checked("systemctl", &["--user", "stop", &name])
    }
    #[cfg(windows)]
    {
        powershell(&format!(
            "$ErrorActionPreference='Stop'; if(Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue){{Stop-ScheduledTask -TaskName {}}}",
            ps(&name),
            ps(&name)
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err("unsupported login service".into())
    }
}
pub fn remove(root: &Path, team: &str) -> Result<()> {
    if !configured(root, team)? {
        return Ok(());
    }
    pause(root, team)?;
    let name = label(root, team)?;
    #[cfg(windows)]
    powershell(&format!(
        "$ErrorActionPreference='Stop'; if(Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue){{Unregister-ScheduledTask -TaskName {} -Confirm:$false}}",
        ps(&name),
        ps(&name)
    ))?;
    #[cfg(target_os = "linux")]
    checked("systemctl", &["--user", "disable", &name])?;
    #[cfg(not(windows))]
    {
        let path = job_file(root, team)?;
        if path.exists() {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = name;
    }
    fs::remove_file(runtime::directory(root, team)?.join("login")).map_err(|e| e.to_string())?;
    Ok(())
}
