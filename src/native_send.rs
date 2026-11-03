//! Native writer for agmsg v1.7.0's built-in SQLite driver.
//! This is deliberately not a schema migrator or a replacement for registration,
//! remote sync, daemon delivery, or extension dispatch.
use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde_json::Value;
use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

fn json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON in {}: {e}", path.display()))
}

fn home() -> Result<PathBuf, String> {
    let value = if cfg!(windows) {
        env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"))
    } else {
        env::var_os("HOME")
    };
    value
        .map(PathBuf::from)
        .ok_or_else(|| "user home is unavailable".into())
}

// Git Bash paths can also be supplied in AGMSG_CONFIG / AGMSG_STORAGE_PATH.
fn native_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let b = text.as_bytes();
    if cfg!(windows) && b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b'/' {
        PathBuf::from(format!("{}:{}", b[1] as char, &text[2..]))
    } else {
        path.to_owned()
    }
}

fn team_name(team: &str) -> Result<(), String> {
    if team.is_empty()
        || matches!(team, "." | "..")
        || team.starts_with('-')
        || team.contains(['/', '\\'])
        || team.chars().any(char::is_control)
        || (cfg!(windows) && team.contains([':', '*', '?', '"', '<', '>', '|']))
    {
        return Err("invalid team name: expected a safe, nonempty path segment".into());
    }
    Ok(())
}

// Use the same exclusion marker as registry-lock.sh. Never reclaim another
// owner's lock. A crash leaves a conservative lock requiring operator recovery.
struct TeamLock(PathBuf);
impl TeamLock {
    fn acquire(team_dir: &Path) -> Result<Self, String> {
        let path = team_dir.join(".config.lock");
        let start = std::time::Instant::now();
        loop {
            match fs::create_dir(&path) {
                Ok(()) => break,
                Err(e)
                    if e.kind() == io::ErrorKind::AlreadyExists
                        && start.elapsed() < Duration::from_secs(10) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    return Err(format!(
                        "cannot lock team registry (retry after other operations finish): {e}"
                    ));
                }
            }
        }
        if let Err(e) = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join(".owner"))
        {
            let _ = fs::remove_dir(&path);
            return Err(format!("cannot claim team registry lock: {e}"));
        }
        let lock = Self(path);
        if team_dir.join(".config.lock.holder").exists() {
            return Err(
                "team registry has a stale holder record; use official recovery before native send"
                    .into(),
            );
        }
        Ok(lock)
    }
}
impl Drop for TeamLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join(".owner"));
        let _ = fs::remove_dir(&self.0);
    }
}

pub fn send(
    root: &Path,
    team: &str,
    from: &str,
    to: &str,
    body_file: Option<&Path>,
) -> Result<(), String> {
    let mut body = String::new();
    if let Some(path) = body_file {
        body = fs::read_to_string(path)
            .map_err(|e| format!("cannot read UTF-8 body file {}: {e}", path.display()))?;
    } else {
        io::stdin()
            .read_to_string(&mut body)
            .map_err(|e| format!("cannot read UTF-8 stdin: {e}"))?;
    }
    // send.sh's command substitution strips LF only, not CR or other whitespace.
    let body = body.trim_end_matches('\n');
    if body.is_empty() {
        return Err("the message body is empty — nothing to send".into());
    }
    if body.contains('\0') {
        return Err("message body contains NUL; refusing lossy shell-compatible delivery".into());
    }
    let storage_config = match env::var_os("AGMSG_STORAGE_DRIVER").filter(|x| !x.is_empty()) {
        Some(driver) => driver.to_string_lossy().into_owned(),
        None => {
            let path = match env::var_os("AGMSG_CONFIG").filter(|x| !x.is_empty()) {
                Some(p) => native_path(Path::new(&p)),
                None => home()?.join(".agents/agmsg/config.json"),
            };
            match fs::metadata(&path) {
                Ok(_) => json(&path)?
                    .get("storage")
                    .and_then(Value::as_str)
                    .unwrap_or("sqlite")
                    .to_owned(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => "sqlite".into(),
                Err(e) => return Err(format!("cannot inspect storage configuration: {e}")),
            }
        }
    };
    let base = env::var_os("AGMSG_STORAGE_PATH")
        .filter(|x| !x.is_empty())
        .map(|x| native_path(Path::new(&x)))
        .unwrap_or_else(|| root.join("db"));
    send_to_store(root, &base, &storage_config, team, from, to, body)
}

fn send_to_store(
    root: &Path,
    base: &Path,
    driver: &str,
    team: &str,
    from: &str,
    to: &str,
    body: &str,
) -> Result<(), String> {
    team_name(team)?;
    let version = fs::read_to_string(root.join("VERSION"))
        .map_err(|e| format!("cannot verify agmsg version: {e}"))?;
    if version.trim().trim_start_matches('v') != "1.7.0" {
        return Err(
            "native send supports agmsg v1.7.0 only; verify compatibility before upgrading".into(),
        );
    }
    if driver != "sqlite" {
        return Err(format!(
            "native send does not support storage driver '{driver}'"
        ));
    }
    if env::var_os("AGMSG_PLUGIN_DIRS").is_some_and(|x| !x.is_empty()) {
        return Err("native send does not support external driver search paths".into());
    }
    for file in [
        "storage/sqlite.sh",
        "partition/shared.sh",
        "partition/per-team.sh",
    ] {
        if root.join("plugins").join(file).exists() {
            return Err("native send does not support overridden storage/partition drivers".into());
        }
    }
    let team_dir = root.join("teams").join(team);
    let _lock = TeamLock::acquire(&team_dir)?;
    let cfg = json(&team_dir.join("config.json"))?;
    let agents = cfg
        .get("agents")
        .and_then(Value::as_object)
        .ok_or("invalid team roster")?;
    for (role, name) in [("from", from), ("to", to)] {
        let member = agents
            .get(name)
            .filter(|x| !x.is_null())
            .ok_or_else(|| format!("{role} agent '{name}' is not registered in team '{team}'"))?;
        // Other types may dispatch _message.sh after commit. Do not silently
        // skip those hooks, nor invoke a shell from the native path.
        let mut types = Vec::new();
        if let Some(t) = member.get("type").and_then(Value::as_str) {
            types.push(t);
        }
        if let Some(regs) = member.get("registrations").and_then(Value::as_array) {
            types.extend(
                regs.iter()
                    .filter_map(|r| r.get("type").and_then(Value::as_str)),
            );
        }
        if role == "to" && types.iter().any(|t| !matches!(*t, "codex" | "claude-code")) {
            return Err("native send currently supports Codex/Claude recipients only (no extension dispatch)".into());
        }
    }
    let partition = match cfg.pointer("/drivers/partition") {
        None | Some(Value::Null) => "shared",
        Some(Value::String(s)) if s.is_empty() => "shared",
        Some(Value::String(s)) => s,
        _ => return Err("invalid partition driver".into()),
    };
    let path = match partition {
        "shared" => base.join("messages.db"),
        "per-team" => base.join("teams").join(team).join("messages.db"),
        other => {
            return Err(format!(
                "native send does not support partition driver '{other}'"
            ));
        }
    };
    // No CREATE flag: never create an empty file at a guessed path.
    let mut db =
        Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|e| {
            format!(
                "cannot open initialized agmsg store {}: {e}",
                path.display()
            )
        })?;
    let timeout = match env::var("AGMSG_BUSY_TIMEOUT") {
        Ok(s) if !s.is_empty() => s
            .parse::<u64>()
            .map_err(|_| "AGMSG_BUSY_TIMEOUT must be milliseconds")?,
        _ => 5000,
    };
    if timeout > i32::MAX as u64 {
        return Err("AGMSG_BUSY_TIMEOUT exceeds SQLite's supported range".into());
    }
    db.busy_timeout(Duration::from_millis(timeout))
        .map_err(|e| e.to_string())?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| format!("cannot start send transaction: {e}"))?;
    let revision: i64 = tx
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mode: String = tx
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let cursor_schema: Option<String> = tx
        .query_row(
            "SELECT value FROM storage_metadata WHERE key='read_cursor_v1'",
            [],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                Ok(None)
            } else {
                Err(e)
            }
        })
        .map_err(|e| {
            format!("agmsg store must be initialized by official inbox/storage_init first: {e}")
        })?;
    if revision != 1 || mode != "wal" || cursor_schema.as_deref() != Some("1") {
        return Err("unsupported/uninitialized agmsg store; expected schema revision 1, WAL and read_cursor_v1=1".into());
    }
    let id = uuid::Uuid::now_v7().to_string();
    let at: String = tx
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO messages(team,from_agent,to_agent,body,created_at) VALUES (?1,?2,?3,?4,?5)",
        params![team, from, to, body, at],
    )
    .map_err(|e| format!("cannot write message: {e}"))?;
    let legacy_id = tx.last_insert_rowid();
    tx.execute("INSERT INTO events(type,id,team,from_agent,to_agent,body,at,legacy_id) VALUES ('message_sent',?1,?2,?3,?4,?5,?6,?7)", params![id,team,from,to,body,at,legacy_id])
        .map_err(|e| format!("cannot write message event (send rolled back): {e}"))?;
    tx.commit()
        .map_err(|e| format!("cannot commit send: {e}"))?;
    Ok(())
}
