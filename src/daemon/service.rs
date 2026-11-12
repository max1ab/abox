//! User-scoped login supervision. Paths/arguments are data, never shell code.
use super::{Result, directory};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
fn label(root: &Path) -> Result<String> {
    let text = format!(
        "{}\0daemon",
        root.to_str().ok_or("installation path is not UTF-8")?
    );
    let hash = ring::digest::digest(&ring::digest::SHA256, text.as_bytes());
    Ok(format!(
        "io.agent-mail.daemon.{}",
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
pub(super) fn configured(root: &Path) -> Result<bool> {
    Ok(directory(root).join("login").is_file())
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
#[cfg(any(target_os = "macos", test))]
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
#[cfg(any(windows, test))]
fn ps(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}
#[cfg(any(windows, test))]
fn scheduler_executable(text: &str) -> String {
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    }
}
/// Rust 1.98 Command inherits every inheritable parent handle on Windows,
/// including PowerShell's extra pipeline handles. Start a fresh launcher with
/// inheritance explicitly off; that launcher opens its own engine log before
/// starting the worker. No shell or PID-based lifecycle is involved.
#[cfg(windows)]
pub(super) fn spawn_detached(root: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
            STARTUPINFOW,
        },
    };
    let executable = env::current_exe().map_err(|e| e.to_string())?;
    let path = executable.to_str().ok_or("executable path is not UTF-8")?;
    let root = root.to_str().ok_or("installation path is not UTF-8")?;
    let line = [path, "--data-dir", root, "service", "launch"]
        .iter()
        .map(|a| win_arg(a))
        .collect::<Vec<_>>()
        .join(" ");
    let application: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut arguments: Vec<u16> = line.encode_utf16().chain(Some(0)).collect();
    // All pointers refer to live buffers for this synchronous call. Null
    // security attributes use the caller's token; false handle inheritance
    // prevents pipeline/lock leaks. Returned handles are owned and closed here.
    unsafe {
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            application.as_ptr(),
            arguments.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        ) == 0
        {
            return Err(format!(
                "cannot start detached Abox daemon: {}",
                std::io::Error::last_os_error()
            ));
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}
#[cfg(any(windows, test))]
fn win_arg(text: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in text.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        if c == '"' {
            result.push_str(&"\\".repeat(slashes * 2 + 1));
            result.push('"');
        } else {
            result.push_str(&"\\".repeat(slashes));
            result.push(c);
        }
        slashes = 0;
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}
#[cfg(windows)]
fn powershell(script: &str) -> Result<()> {
    checked(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}
fn job_file(root: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(home()?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", label(root)?)))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(home()?
            .join(".config/systemd/user")
            .join(format!("{}.service", label(root)?)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Ok(directory(root).join("login-job"))
    }
}
pub fn install(root: &Path) -> Result<()> {
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_str().ok_or("executable path is not UTF-8")?;
    let root_text = root.to_str().ok_or("installation path is not UTF-8")?;
    if [exe, root_text].iter().any(|s| s.contains(['\n', '\r'])) {
        return Err("login service paths cannot contain newlines".into());
    }
    let name = label(root)?;
    let path = job_file(root)?;
    if path.exists() && !configured(root)? {
        return Err("login job path already exists; refusing to overwrite an unowned job".into());
    }
    #[cfg(target_os = "macos")]
    {
        fs::create_dir_all(path.parent().ok_or("missing launch agent directory")?)
            .map_err(|e| e.to_string())?;
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{}</string><string>--data-dir</string><string>{}</string><string>service</string><string>launch</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer></dict></plist>",
            xml(&name),
            xml(exe),
            xml(root_text)
        );
        crate::registry::atomic(&path, text.as_bytes())?;
    }
    #[cfg(target_os = "linux")]
    {
        fs::create_dir_all(path.parent().ok_or("missing systemd user directory")?)
            .map_err(|e| e.to_string())?;
        let quote = |s: &str| {
            format!(
                "\"{}\"",
                s.replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('$', "$$")
                    .replace('%', "%%")
            )
        };
        let text = format!(
            "[Unit]\nDescription=Abox background service\n[Service]\nExecStart={} --data-dir {} service launch\nRestart=on-failure\nRestartSec=5\n[Install]\nWantedBy=default.target\n",
            quote(exe),
            quote(root_text)
        );
        crate::registry::atomic(&path, text.as_bytes())?;
        checked("systemctl", &["--user", "daemon-reload"])?;
        checked("systemctl", &["--user", "enable", &name])?;
    }
    #[cfg(windows)]
    {
        let exe = scheduler_executable(exe);
        let arguments = ["--data-dir", root_text, "service", "launch"]
            .iter()
            .map(|a| win_arg(a))
            .collect::<Vec<_>>()
            .join(" ");
        powershell(&format!(
            "$ErrorActionPreference='Stop'; if(Get-ScheduledTask -TaskName {} -ErrorAction SilentlyContinue){{throw 'Task already exists; refusing to overwrite it'}}; $a=New-ScheduledTaskAction -Execute {} -Argument {}; $t=New-ScheduledTaskTrigger -AtLogOn -User ([Security.Principal.WindowsIdentity]::GetCurrent().Name); $p=New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited; $s=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1); Register-ScheduledTask -TaskName {} -Action $a -Trigger $t -Principal $p -Settings $s | Out-Null",
            ps(&name),
            ps(&exe),
            ps(&arguments),
            ps(&name)
        ))?;
    }
    crate::registry::atomic(&directory(root).join("login"), b"1\n")?;
    resume(root)
}
pub fn resume(root: &Path) -> Result<()> {
    let name = label(root)?;
    #[cfg(target_os = "macos")]
    {
        let domain = domain()?;
        let target = format!("{domain}/{name}");
        checked("launchctl", &["enable", &target])?;
        if checked("launchctl", &["kickstart", &target]).is_err() {
            checked(
                "launchctl",
                &["bootstrap", &domain, &job_file(root)?.to_string_lossy()],
            )?;
        }
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        checked("systemctl", &["--user", "start", &name])
    }
    #[cfg(windows)]
    {
        powershell(&format!(
            "$ErrorActionPreference='Stop'; Start-ScheduledTask -TaskName {}",
            ps(&name)
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err("login supervision is not supported on this platform".into())
    }
}
pub fn pause(root: &Path) -> Result<()> {
    if !configured(root)? {
        return Ok(());
    }
    let name = label(root)?;
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
pub fn remove(root: &Path) -> Result<()> {
    if !configured(root)? {
        return Ok(());
    }
    pause(root)?;
    let name = label(root)?;
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
        let path = job_file(root)?;
        if path.exists() {
            fs::remove_file(path).map_err(|e| e.to_string())?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = name;
    }
    fs::remove_file(directory(root).join("login")).map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_arguments_and_xml_are_data_not_expressions() {
        assert_eq!(
            win_arg("C:\\path with spaces\\"),
            "\"C:\\path with spaces\\\\\""
        );
        assert_eq!(ps("O'Brien"), "'O''Brien'");
        assert_eq!(
            scheduler_executable(r"\\?\C:\Users\Test User\agent-mail.exe"),
            r"C:\Users\Test User\agent-mail.exe"
        );
        assert_eq!(
            scheduler_executable(r"\\?\UNC\host\share\agent-mail.exe"),
            r"\\host\share\agent-mail.exe"
        );
        assert_eq!(xml("<&\""), "&lt;&amp;&quot;");
        assert_ne!(
            label(Path::new("/one")).unwrap(),
            label(Path::new("/two")).unwrap()
        );
    }
}
