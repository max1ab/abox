//! Local IPC for the native sync engine. A kernel file lock provides exclusion;
//! a loopback datagram with a private token is only a wake hint. Persistent
//! SQLite reservations, not this endpoint file, remain the source of truth.
use super::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU8},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

pub(crate) fn directory(root: &Path, team: &str) -> Result<PathBuf> {
    crate::store::team_name(team)?;
    Ok(root.join("run/native-sync").join(team))
}
pub(super) fn prepare_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| format!("cannot create native sync runtime directory: {e}"))
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = private_options()
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, value).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 {
        return Err("native sync runtime record is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid native sync runtime record".into())
}

pub(super) struct EngineLock {
    _file: File,
}
pub(crate) struct ControlLock {
    _file: File,
}
impl ControlLock {
    pub(crate) fn acquire(root: &Path, team: &str) -> Result<Self> {
        let dir = directory(root, team)?;
        prepare_directory(&dir)?;
        let file = private_options()
            .read(true)
            .create(true)
            .truncate(false)
            .open(dir.join("control.lock"))
            .map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|_| "another sync lifecycle command is in progress; retry shortly")?;
        Ok(Self { _file: file })
    }
}
impl EngineLock {
    pub(super) fn acquire(root: &Path, team: &str) -> Result<Self> {
        let dir = directory(root, team)?;
        prepare_directory(&dir)?;
        let file = private_options()
            .read(true)
            .create(true)
            .truncate(false)
            .open(dir.join("engine.lock"))
            .map_err(|e| e.to_string())?;
        file.try_lock().map_err(|_| {
            "another native sync operation is already running for this team".to_owned()
        })?;
        Ok(Self { _file: file })
    }
}
#[derive(Deserialize, Serialize)]
struct Endpoint {
    address: SocketAddr,
    token: String,
}
impl Endpoint {
    fn valid(&self) -> bool {
        self.address.ip().is_loopback()
            && self.address.port() != 0
            && uuid::Uuid::parse_str(&self.token).is_ok()
    }
}
struct Published {
    path: PathBuf,
    token: String,
}
impl Drop for Published {
    fn drop(&mut self) {
        if read_json::<Endpoint>(&self.path).is_ok_and(|e| e.token == self.token) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Send failure never turns an already-committed send into a reported failure.
pub(crate) fn notify(root: &Path, team: &str) {
    crate::daemon::wake(root);
    let result = (|| -> Result<()> {
        let endpoint: Endpoint = read_json(&directory(root, team)?.join("endpoint.json"))?;
        if !endpoint.valid() {
            return Ok(());
        }
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        socket
            .send_to(
                format!("wake {}", endpoint.token).as_bytes(),
                endpoint.address,
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = result;
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Status {
    team: String,
    pub(super) running: bool,
    phase: String,
    last_success_ms: Option<u64>,
    last_error: Option<String>,
    uploaded: u64,
    downloaded: u64,
    transport_cursor: Option<String>,
    #[serde(default)]
    remote_events: String,
}
impl Status {
    fn new(team: &str) -> Self {
        Self {
            team: team.into(),
            running: false,
            phase: "stopped".into(),
            last_success_ms: None,
            last_error: None,
            uploaded: 0,
            downloaded: 0,
            transport_cursor: None,
            remote_events: "stopped".into(),
        }
    }
}
/// A process ID or a stale status file is not evidence that an engine is live.
pub fn status(root: &Path, team: &str) -> Result<Status> {
    let dir = directory(root, team)?;
    let mut status: Status = match read_json(&dir.join("status.json")) {
        Ok(status) => status,
        Err(_) => Status::new(team),
    };
    status.running = false;
    if let Ok(endpoint) = read_json::<Endpoint>(&dir.join("endpoint.json"))
        && endpoint.valid()
    {
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_millis(300)))
            .map_err(|e| e.to_string())?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let _ = socket.send_to(
            format!("ping {} {nonce}", endpoint.token).as_bytes(),
            endpoint.address,
        );
        let mut buffer = [0u8; 128];
        if let Ok((n, source)) = socket.recv_from(&mut buffer) {
            status.running =
                source == endpoint.address && &buffer[..n] == format!("pong {nonce}").as_bytes();
        }
    }
    if !status.running {
        status.phase = "stopped".into();
    } else if status.phase == "stopped" {
        status.phase = "starting".into();
    }
    Ok(status)
}
struct AbortOnDrop(tokio::task::AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn listen(
    socket: tokio::net::UdpSocket,
    token: String,
    wake: Arc<Notify>,
    stop: Arc<Notify>,
) -> Result<()> {
    let mut buffer = [0u8; 256];
    loop {
        let (n, source) = socket
            .recv_from(&mut buffer)
            .await
            .map_err(|_| "native IPC listener failed")?;
        if !source.ip().is_loopback() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&buffer[..n]) else {
            continue;
        };
        let parts: Vec<_> = text.split_whitespace().collect();
        match parts.as_slice() {
            ["wake", given] if *given == token => wake.notify_one(),
            ["stop", given] if *given == token => stop.notify_one(),
            ["ping", given, nonce] if *given == token && uuid::Uuid::parse_str(nonce).is_ok() => {
                let _ = socket
                    .send_to(format!("pong {nonce}").as_bytes(), source)
                    .await;
            }
            _ => {}
        }
    }
}

pub async fn run(root: &Path, team: &str, interval: Duration) -> Result<()> {
    log_lifecycle(team, "engine.start", None);
    let result = run_engine(root, team, interval).await;
    if let Err(error) = &result {
        log_lifecycle(team, "engine.error", Some(error));
    }
    log_lifecycle(
        team,
        "engine.exit",
        Some(if result.is_ok() { "clean" } else { "error" }),
    );
    result
}

fn log_lifecycle(team: &str, event: &str, reason: Option<&str>) {
    eprintln!(
        "{}",
        serde_json::json!({
            "at_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
            "pid": std::process::id(),
            "team": team,
            "event": event,
            "reason": reason,
        })
    );
}

async fn run_engine(root: &Path, team: &str, interval: Duration) -> Result<()> {
    let _lock = EngineLock::acquire(root, team)?;
    // Invalid local bindings fail before publishing a live endpoint.
    drop(Context::open(root, team)?);
    let dir = directory(root, team)?;
    eprintln!("Native sync running for {team}; Ctrl+C to stop.");
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let token = uuid::Uuid::new_v4().to_string();
    let address = socket.local_addr().map_err(|e| e.to_string())?;
    let wake = Arc::new(Notify::new());
    let stop = Arc::new(Notify::new());
    #[cfg(unix)]
    let _ignore_hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
        .map_err(|e| e.to_string())?;
    let _published = Published {
        path: dir.join("endpoint.json"),
        token: token.clone(),
    };
    write_json(
        &_published.path,
        &Endpoint {
            address,
            token: token.clone(),
        },
    )?;
    let mut listener = tokio::spawn(listen(socket, token, wake.clone(), stop.clone()));
    let _abort = AbortOnDrop(listener.abort_handle());
    let event_mode = Arc::new(AtomicU8::new(0));
    let event_task = tokio::spawn(super::events::follow(
        root.to_owned(),
        team.to_owned(),
        wake.clone(),
        event_mode.clone(),
    ));
    let _event_abort = AbortOnDrop(event_task.abort_handle());
    let mut state = Status::new(team);
    state.running = true;
    let status_path = dir.join("status.json");
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut failures = 0u32;
    let result=async {
        loop {
            state.phase="syncing".into();
            write_json(&status_path,&state)?;
            let outcome=tokio::select! {
                biased;
                reason=&mut shutdown=>{
                    log_lifecycle(team,"engine.stop_requested",Some(reason));
                    break;
                },
                _=stop.notified()=>{
                    log_lifecycle(team,"engine.stop_requested",Some("IPC stop"));
                    break;
                },
                stopped=&mut listener=>return Err(format!("native IPC listener stopped: {stopped:?}")),
                result=super::cycle(root,team)=>result,
            };
            let (delay,catch_up)=match outcome {
                Ok(outcome)=>{
                    crate::daemon::wake(root);
                    failures=0;
                    state.last_success_ms=Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64);
                    state.last_error=None;
                    state.uploaded+=outcome.uploaded as u64;
                    state.downloaded+=outcome.downloaded as u64;
                    state.transport_cursor=Some(outcome.transport_cursor);
                    state.phase="idle".into();
                    (interval,outcome.has_more)
                },
                Err(error)=>{
                    failures=failures.saturating_add(1);
                    eprintln!("Native sync failed: {error}; retrying.");
                    state.phase="retrying".into();
                    state.last_error=Some(error);
                    (Duration::from_secs(1u64<<failures.saturating_sub(1).min(6)),false)
                },
            };
            state.remote_events=super::events::mode_name(&event_mode);
            write_json(&status_path,&state)?;
            if catch_up { tokio::task::yield_now().await;continue; }
            tokio::select! {
                biased;
                reason=&mut shutdown=>{
                    log_lifecycle(team,"engine.stop_requested",Some(reason));
                    break;
                },
                _=stop.notified()=>{
                    log_lifecycle(team,"engine.stop_requested",Some("IPC stop"));
                    break;
                },
                stopped=&mut listener=>return Err(format!("native IPC listener stopped: {stopped:?}")),
                _=wake.notified()=>{},
                _=tokio::time::sleep(delay)=>{},
            }
        }
        Ok(())
    }.await;
    listener.abort();
    event_task.abort();
    state.remote_events = "stopped".into();
    state.running = false;
    state.phase = "stopped".into();
    // A status-write failure must not hide an earlier transport/IPC failure.
    let saved = write_json(&status_path, &state);
    result.and(saved)
}
pub(crate) async fn shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            return tokio::select! {
                _=tokio::signal::ctrl_c()=>"Ctrl+C",
                _=term.recv()=>"SIGTERM",
            };
        }
    }
    let _ = tokio::signal::ctrl_c().await;
    "Ctrl+C"
}

/// Spawn without inheriting agent/SSH output pipes. The child, not its PID or
/// this return value, proves readiness by publishing its authenticated IPC.
pub fn start(root: &Path, team: &str) -> Result<serde_json::Value> {
    let _control = ControlLock::acquire(root, team)?;
    let dir = directory(root, team)?;
    if !dir.join("engine").is_file() {
        return Err("select native sync first with: agent-mail sync enable TEAM".into());
    }
    if status(root, team)?.running {
        return Ok(serde_json::json!({"state":"running"}));
    }
    if super::service::configured(root, team)? {
        super::service::resume(root, team)?;
        return wait_ready(root, team);
    }
    prepare_directory(&dir)?;
    spawn_background(root, team)?;
    wait_ready(root, team)
}
#[cfg(windows)]
fn spawn_background(root: &Path, team: &str) -> Result<()> {
    super::service::spawn_detached(root, team)
}
#[cfg(not(windows))]
fn spawn_background(root: &Path, team: &str) -> Result<()> {
    let dir = directory(root, team)?;
    let log = private_options()
        .create(true)
        .append(true)
        .open(dir.join("engine.log"))
        .map_err(|e| e.to_string())?;
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .args(["--agmsg-dir"])
        .arg(root)
        .args(["sync", "run", team]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    if let Ok(env) =
        read_json::<std::collections::BTreeMap<String, String>>(&dir.join("environment.json"))
    {
        for (name, value) in env {
            if super::bridge::ENVIRONMENT.contains(&name.as_str()) {
                command.env(name, value);
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .spawn()
        .map_err(|e| format!("cannot start native sync: {e}"))?;
    Ok(())
}

fn wait_ready(root: &Path, team: &str) -> Result<serde_json::Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if status(root, team)?.running {
            return Ok(serde_json::json!({"state":"running"}));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("native sync did not become ready; inspect run/native-sync/TEAM/engine.log and retry sync start".into())
}

pub async fn stop(root: &Path, team: &str) -> Result<()> {
    let dir = directory(root, team)?;
    if !dir.exists() {
        return Ok(());
    }
    if let Ok(lock) = EngineLock::acquire(root, team) {
        drop(lock);
        return Ok(());
    }
    let endpoint: Endpoint = read_json(&dir.join("endpoint.json"))?;
    if !endpoint.valid() {
        return Err("invalid native control endpoint".into());
    }
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    for _ in 0..100 {
        socket
            .send_to(
                format!("stop {}", endpoint.token).as_bytes(),
                endpoint.address,
            )
            .map_err(|e| e.to_string())?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(lock) = EngineLock::acquire(root, team) {
            drop(lock);
            return Ok(());
        }
    }
    Err("native sync did not release its engine lock; no process was killed by PID".into())
}

/// Service managers supervise this waiting launcher and its child. Storage
/// environment is restored on the child without mutating a threaded process.
pub fn launch(root: &Path, team: &str) -> Result<()> {
    let dir = directory(root, team)?;
    prepare_directory(&dir)?;
    let log = private_options()
        .create(true)
        .append(true)
        .open(dir.join("engine.log"))
        .map_err(|e| e.to_string())?;
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--agmsg-dir")
        .arg(root)
        .args(["sync", "run", team]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    let values =
        read_json::<std::collections::BTreeMap<String, String>>(&dir.join("environment.json"))?;
    for (name, value) in values {
        if super::bridge::ENVIRONMENT.contains(&name.as_str()) {
            command.env(name, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("native sync exited; inspect its engine.log".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kernel_lock_excludes_another_engine_and_recovers_without_deleting_files() {
        let temp = tempfile::tempdir().unwrap();
        let lock = EngineLock::acquire(temp.path(), "dev").unwrap();
        assert!(EngineLock::acquire(temp.path(), "dev").is_err());
        drop(lock);
        assert!(EngineLock::acquire(temp.path(), "dev").is_ok());
    }
    #[test]
    fn stopped_status_and_missing_notifications_do_not_create_runtime_files() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!status(temp.path(), "dev").unwrap().running);
        notify(temp.path(), "dev");
        assert!(!temp.path().join("run").exists());
        assert!(status(temp.path(), "../escape").is_err());
    }
    #[test]
    fn stale_status_does_not_claim_the_engine_is_running() {
        let temp = tempfile::tempdir().unwrap();
        let dir = directory(temp.path(), "dev").unwrap();
        prepare_directory(&dir).unwrap();
        let mut state = Status::new("dev");
        state.running = true;
        state.phase = "syncing".into();
        write_json(&dir.join("status.json"), &state).unwrap();
        let observed = status(temp.path(), "dev").unwrap();
        assert!(!observed.running);
        assert_eq!(observed.phase, "stopped");
    }
}
