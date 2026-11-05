//! One resident process: independent per-team sync tasks and local notifications.
mod service;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::task::JoinHandle;
pub(crate) type Result<T> = std::result::Result<T, String>;
pub(crate) fn directory(root: &Path) -> PathBuf {
    root.join("run/daemon")
}
fn prepare(root: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory(root)).map_err(|e| e.to_string())
}
fn lock(root: &Path, name: &str) -> Result<File> {
    prepare(root)?;
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory(root).join(name))
        .map_err(|e| e.to_string())?;
    f.try_lock()
        .map_err(|_| "another daemon operation is running")?;
    Ok(f)
}
#[derive(Serialize, Deserialize)]
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
fn endpoint(root: &Path) -> Result<Endpoint> {
    let file = File::open(directory(root).join("endpoint.json")).map_err(|e| e.to_string())?;
    let mut b = Vec::new();
    file.take(4097)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() > 4096 {
        return Err("invalid daemon endpoint size".into());
    }
    let e: Endpoint = serde_json::from_slice(&b).map_err(|_| "invalid daemon endpoint")?;
    if !e.valid() {
        return Err("invalid daemon endpoint".into());
    }
    Ok(e)
}
struct Published {
    root: PathBuf,
    token: String,
}
impl Drop for Published {
    fn drop(&mut self) {
        if endpoint(&self.root).is_ok_and(|e| e.token == self.token) {
            let _ = fs::remove_file(directory(&self.root).join("endpoint.json"));
        }
    }
}
pub(crate) fn wake(root: &Path) {
    let _ = send(root, "wake");
}
fn send(root: &Path, action: &str) -> Result<()> {
    let e = endpoint(root)?;
    let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    socket
        .send_to(format!("{action} {}", e.token).as_bytes(), e.address)
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub(crate) fn status(root: &Path) -> Result<Value> {
    let mut state = match fs::read(directory(root).join("status.json")) {
        Ok(b) => serde_json::from_slice::<Value>(&b).map_err(|_| "invalid daemon status")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"phase":"stopped"}),
        Err(e) => return Err(e.to_string()),
    };
    let active = || -> Result<bool> {
        let e = endpoint(root)?;
        let socket = UdpSocket::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| e.to_string())?;
        let nonce = uuid::Uuid::new_v4().to_string();
        socket
            .send_to(format!("ping {} {nonce}", e.token).as_bytes(), e.address)
            .map_err(|e| e.to_string())?;
        let mut buf = [0u8; 128];
        let (n, peer) = socket.recv_from(&mut buf).map_err(|e| e.to_string())?;
        Ok(peer == e.address && &buf[..n] == format!("pong {nonce}").as_bytes())
    };
    let running = active().unwrap_or(false);
    state["running"] = json!(running);
    if !running {
        state["phase"] = json!("stopped");
    } else if state["phase"] == "stopped" {
        state["phase"] = json!("starting");
    }
    state["supervised"] = json!(service::configured(root)?);
    Ok(state)
}
/// No simultaneous notification engines. Reading legacy ownership is safe;
/// we never alter a foreign PID, queue, install manifest, or daemon intent.
pub(crate) fn check_legacy(root: &Path) -> Result<()> {
    let path = root.join("run/install.db");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    let (desired, state): (Option<String>, String) = db
        .query_row(
            "SELECT desired,state FROM daemon_intent CROSS JOIN daemon_owner",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "cannot verify old agmsgd ownership; inspect its status before takeover")?;
    if desired.as_deref() == Some("on") || state != "none" {
        return Err("agmsgd is still enabled or owns this installation; disable it using the existing installation before starting the Rust daemon".into());
    }
    Ok(())
}
const ENVIRONMENT: &[&str] = &[
    "AGMSG_CONFIG",
    "AGMSG_STORAGE_PATH",
    "AGMSG_STORAGE_DRIVER",
    "AGMSG_BUSY_TIMEOUT",
    "AGENT_MAIL_CODEX",
    "PATH",
];
pub(crate) async fn start(root: &Path, no_login: bool) -> Result<Value> {
    let _control = lock(root, "control.lock")?;
    check_legacy(root)?;
    if status(root)?["running"] == true {
        if no_login || service::configured(root)? {
            return status(root);
        }
        // Switching a detached process to OS supervision needs a real handoff,
        // otherwise the manager would repeatedly launch an excluded duplicate.
        send(root, "stop")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while status(root)?["running"] == true {
            if Instant::now() >= deadline {
                return Err("detached daemon did not stop for supervisor handoff".into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let mut environment: BTreeMap<String, String> = BTreeMap::new();
    for name in ENVIRONMENT {
        if let Ok(mut value) = std::env::var(name) {
            if matches!(
                *name,
                "AGMSG_CONFIG" | "AGMSG_STORAGE_PATH" | "AGENT_MAIL_CODEX"
            ) && !value.is_empty()
            {
                let p = crate::store::native_path(Path::new(&value));
                if !p.is_absolute() && (*name != "AGENT_MAIL_CODEX" || p.components().count() > 1) {
                    value = std::env::current_dir()
                        .map_err(|e| e.to_string())?
                        .join(p)
                        .to_string_lossy()
                        .into_owned();
                }
            }
            environment.insert((*name).into(), value);
        }
    }
    crate::registry::atomic(
        &directory(root).join("environment.json"),
        serde_json::to_vec(&environment)
            .map_err(|e| e.to_string())?
            .as_slice(),
    )?;
    // Stop only legacy Rust jobs owned by this exact root. Official Node
    // transports are rejected separately by each team's synchronization guard.
    for team in crate::mailbox::teams(root)? {
        let name = team["name"].as_str().ok_or("invalid team name")?;
        crate::sync::service::remove(root, name)?;
        crate::sync::runtime::stop(root, name).await?;
    }
    if !no_login {
        service::install(root)?;
    } else {
        spawn(root)?;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let state = status(root)?;
        if state["running"] == true {
            return Ok(state);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("daemon did not become ready; inspect run/daemon/daemon.log".into())
}
#[cfg(not(windows))]
fn spawn(root: &Path) -> Result<()> {
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory(root).join("daemon.log"))
        .map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    cmd.arg("--agmsg-dir")
        .arg(root)
        .args(["daemon", "launch"])
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
        .map_err(|e| format!("cannot launch daemon: {e}"))?;
    Ok(())
}
#[cfg(windows)]
fn spawn(root: &Path) -> Result<()> {
    service::spawn_detached(root)
}
pub(crate) async fn stop(root: &Path) -> Result<()> {
    let _control = lock(root, "control.lock")?;
    service::remove(root)?;
    if status(root)?["running"] != true {
        return Ok(());
    }
    send(root, "stop")?;
    for _ in 0..100 {
        if status(root)?["running"] != true {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err("daemon is still stopping; inspect its log".into())
}
pub(crate) fn prepare_launch(root: &Path) -> Result<File> {
    let env = fs::read(directory(root).join("environment.json"));
    if let Ok(bytes) = env {
        let values: BTreeMap<String, String> =
            serde_json::from_slice(&bytes).map_err(|_| "invalid daemon environment")?;
        for (key, value) in values {
            if ENVIRONMENT.contains(&key.as_str()) {
                // Only invoked by main before the Tokio runtime or other threads.
                unsafe {
                    std::env::set_var(key, value);
                }
            }
        }
    }
    prepare(root)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = options
        .open(directory(root).join("daemon.log"))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // The owned file stays alive for main's lifetime; dup2 owns its copies.
        unsafe {
            if libc::dup2(log.as_raw_fd(), 1) < 0 || libc::dup2(log.as_raw_fd(), 2) < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Console::{
            STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };
        unsafe {
            if SetStdHandle(STD_OUTPUT_HANDLE, log.as_raw_handle()) == 0
                || SetStdHandle(STD_ERROR_HANDLE, log.as_raw_handle()) == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
    }
    Ok(log)
}
struct Tasks {
    sync: BTreeMap<String, JoinHandle<Result<()>>>,
    notify: Option<JoinHandle<Result<crate::notify::Report>>>,
}
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in self.sync.values() {
            task.abort();
        }
        if let Some(task) = &self.notify {
            task.abort();
        }
    }
}
pub(crate) async fn run(root: &Path) -> Result<()> {
    check_legacy(root)?;
    let _engine = lock(root, "engine.lock")?;
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let token = uuid::Uuid::new_v4().to_string();
    let endpoint = Endpoint {
        address: socket.local_addr().map_err(|e| e.to_string())?,
        token: token.clone(),
    };
    let _published = Published {
        root: root.into(),
        token: token.clone(),
    };
    crate::registry::atomic(
        &directory(root).join("endpoint.json"),
        serde_json::to_vec(&endpoint)
            .map_err(|e| e.to_string())?
            .as_slice(),
    )?;
    let mut state = json!({"running":true,"phase":"running","pid":std::process::id(),"teams":{},"notifications":null});
    let mut tasks = Tasks {
        sync: BTreeMap::new(),
        notify: None,
    };
    let mut timer = tokio::time::interval(Duration::from_secs(2));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let shutdown = crate::sync::runtime::shutdown_signal();
    tokio::pin!(shutdown);
    let mut buf = [0u8; 256];
    eprintln!(
        "{}",
        json!({"event":"daemon.start","at_ms":crate::notify::now(),"pid":std::process::id()})
    );
    let result=async {
        loop {
            tokio::select!{
                reason=&mut shutdown=>{state["stop_reason"]=json!(reason);break;},
                _=timer.tick()=>{},
                packet=socket.recv_from(&mut buf)=>{
                    let(n,peer)=packet.map_err(|_|"daemon IPC failed")?;
                    if !peer.ip().is_loopback(){continue;}
                    let Ok(text)=std::str::from_utf8(&buf[..n])else{continue;};let parts:Vec<_>=text.split_whitespace().collect();
                    match parts.as_slice(){
                        ["ping",given,nonce]if *given==token&&uuid::Uuid::parse_str(nonce).is_ok()=>{let _=socket.send_to(format!("pong {nonce}").as_bytes(),peer).await;continue;},
                        ["stop",given]if *given==token=>{state["stop_reason"]=json!("IPC stop");break;},
                        ["wake",given]if *given==token=>{},_=>continue,
                    }
                }
            }
            let teams=crate::mailbox::teams(root)?;
            let mut selected=std::collections::BTreeSet::new();
            for team in teams {
                let name=team["name"].as_str().ok_or("invalid team name")?.to_owned();
                let cfg=match crate::registry::config(root,&name){Ok(c)=>c,Err(e)=>{state["teams"][&name]=json!({"error":e});continue;}};
                if cfg["import_pending"]==true||cfg["sync_enabled"]==false||!cfg["remote_binding"].is_object()||!cfg["remote_binding"]["disconnected_at"].is_null(){continue;}
                selected.insert(name.clone());
                if let Some(task)=tasks.sync.get(&name)&&task.is_finished(){
                    let task=tasks.sync.remove(&name).expect("present task");
                    state["teams"][&name]=match task.await{Ok(Err(e))=>json!({"error":e}),Err(_)=>json!({"error":"sync task interrupted"}),_=>json!({"state":"restarting"})};
                }
                if !tasks.sync.contains_key(&name){let path=root.to_owned();let team=name.clone();tasks.sync.insert(name.clone(),tokio::spawn(async move{crate::sync::runtime::run(&path,&team,Duration::from_secs(60)).await}));}
                if state["teams"].get(&name).is_none(){state["teams"][&name]=json!({"state":"running"});}
            }
            tasks.sync.retain(|team,task|{if selected.contains(team){true}else{task.abort();state["teams"].as_object_mut().expect("team object").remove(team);false}});
            if tasks.notify.as_ref().is_some_and(|t|t.is_finished()){
                let task=tasks.notify.take().expect("notification task");state["notifications"]=match task.await{Ok(Ok(report))=>serde_json::to_value(report).map_err(|e|e.to_string())?,Ok(Err(e))=>json!({"error":e}),Err(_)=>json!({"error":"notification task interrupted"})};
            }
            if tasks.notify.is_none(){let path=root.to_owned();tasks.notify=Some(tokio::spawn(async move{crate::notify::once(&path).await}));}
            state["updated_ms"]=json!(crate::notify::now());crate::registry::atomic(&directory(root).join("status.json"),state.to_string().as_bytes())?;
        }
        Ok(())
    }.await;
    state["running"] = json!(false);
    state["phase"] = json!("stopped");
    if let Err(e) = &result {
        state["error"] = json!(e);
    }
    let _ = crate::registry::atomic(
        &directory(root).join("status.json"),
        state.to_string().as_bytes(),
    );
    eprintln!(
        "{}",
        json!({"event":"daemon.exit","at_ms":crate::notify::now(),"reason":state["stop_reason"],"error":state["error"]})
    );
    result
}
