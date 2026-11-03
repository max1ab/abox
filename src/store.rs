//! Shared access to initialized agmsg v1.7.0 SQLite stores.
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{
    env, fs,
    io::{self, Write},
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

pub(crate) fn team_name(team: &str) -> Result<(), String> {
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

// Share the official directory/owner exclusion protocol. A separate kernel
// lock proves when a marker written by this native implementation is orphaned;
// empty/foreign markers and official holder records are never reclaimed here.
pub(crate) struct TeamLock {
    path: PathBuf,
    marker: String,
    _kernel: fs::File,
}
impl TeamLock {
    pub(crate) fn acquire(team_dir: &Path) -> Result<Self, String> {
        let start = std::time::Instant::now();
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let kernel = options
            .open(team_dir.join(".agent-mail-registry.lock"))
            .map_err(|e| e.to_string())?;
        loop {
            match kernel.try_lock() {
                Ok(()) => break,
                Err(fs::TryLockError::WouldBlock) if start.elapsed() < Duration::from_secs(10) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(e) => return Err(format!("cannot lock native registry owner: {e}")),
            }
        }
        let path = team_dir.join(".config.lock");
        loop {
            match fs::create_dir(&path) {
                Ok(()) => break,
                Err(e)
                    if e.kind() == io::ErrorKind::AlreadyExists
                        && start.elapsed() < Duration::from_secs(10) =>
                {
                    let owner = fs::read_to_string(path.join(".owner")).unwrap_or_default();
                    let native = owner
                        .strip_prefix("agent-mail-registry-v1 ")
                        .is_some_and(|token| uuid::Uuid::parse_str(token).is_ok());
                    let only_owner = fs::read_dir(&path).ok().is_some_and(|mut items| {
                        items.all(|entry| entry.is_ok_and(|item| item.file_name() == ".owner"))
                    });
                    if native && only_owner && !team_dir.join(".config.lock.holder").exists() {
                        fs::remove_file(path.join(".owner")).map_err(|e| e.to_string())?;
                        fs::remove_dir(&path).map_err(|e| e.to_string())?;
                        continue;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => {
                    return Err(format!(
                        "cannot lock team registry (retry after other operations finish): {e}"
                    ));
                }
            }
        }
        let marker = format!("agent-mail-registry-v1 {}", uuid::Uuid::new_v4());
        let mut owner = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join(".owner"))
        {
            Ok(file) => file,
            Err(e) => {
                let _ = fs::remove_dir(&path);
                return Err(format!("cannot claim team registry lock: {e}"));
            }
        };
        owner
            .write_all(marker.as_bytes())
            .map_err(|e| e.to_string())?;
        drop(owner);
        let lock = Self {
            path,
            marker,
            _kernel: kernel,
        };
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
        if fs::read_to_string(self.path.join(".owner")).is_ok_and(|value| value == self.marker) {
            let _ = fs::remove_file(self.path.join(".owner"));
            let _ = fs::remove_dir(&self.path);
        }
    }
}

pub(crate) struct TeamStore {
    pub db: Connection,
    pub config: Value,
    pub team_dir: PathBuf,
    pub base: PathBuf,
    pub registry_lock: Option<TeamLock>,
}

pub(crate) fn open(root: &Path, team: &str) -> Result<TeamStore, String> {
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
    open_at(root, &base, &storage_config, team)
}

fn open_at(root: &Path, base: &Path, driver: &str, team: &str) -> Result<TeamStore, String> {
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
    let db =
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
    validate(&db)?;

    Ok(TeamStore {
        db,
        config: cfg,
        team_dir,
        base: base.to_owned(),
        registry_lock: Some(_lock),
    })
}

pub(crate) fn validate(db: &Connection) -> Result<(), String> {
    let revision: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mode: String = db
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let cursor_schema: Option<String> = db
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_marked_orphan_is_recovered_using_the_kernel_owner_lock() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".config.lock");
        fs::create_dir(&path).unwrap();
        fs::write(
            path.join(".owner"),
            format!("agent-mail-registry-v1 {}", uuid::Uuid::new_v4()),
        )
        .unwrap();
        let lock = TeamLock::acquire(temp.path()).unwrap();
        let competing = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(temp.path().join(".agent-mail-registry.lock"))
            .unwrap();
        assert!(matches!(
            competing.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));
        drop(lock);
        assert!(!path.exists());
        assert!(competing.try_lock().is_ok());
    }
}
