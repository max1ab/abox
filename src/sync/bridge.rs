//! Pinned, reversible selection of the native engine from upstream hooks.
//! The official install fence and manifest remain authoritative: never bypass
//! agmsgd's integrity check, and never change its agent delivery adapters.
use super::{Context, Result, runtime, service};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write,
    path::{Component, Path},
    time::Duration,
};

const HELPER: &str = "scripts/remote.sh";
const ORIGINAL: &str = "0f509b887a8b0fa0579c9eaf82a68f521dda3b892d9619b243d97ca5d32f4f39";
const CALL: &str = "cmd_sync_start() {\n";
const REPLACEMENT: &str = r#"cmd_sync_start() {
  # agent-mail native engine bridge v1; unselected teams retain upstream behavior.
  if [ "$#" -eq 1 ]; then
    agmsg_validate_team_name "$1" || return 1
    local _am_marker="$CONNECTION_ROOT/run/native-sync/$1/engine" _am_exe=""
    if [ -f "$_am_marker" ]; then
      IFS= read -r _am_exe < "$_am_marker" || return 1
      [ -x "$_am_exe" ] || { echo 'agent-mail executable is unavailable' >&2; return 1; }
      "$_am_exe" --agmsg-dir "$CONNECTION_ROOT" sync start "$1" || return $?
      echo 'native sync already running'
      return 0
    fi
  fi
"#;
const LOW_LEVEL: &str = "_remote_sync_engine_start() {\n  local team=\"$1\"\n";
const GUARDED: &str = r#"_remote_sync_engine_start() {
  local team="$1"
  # A connected/updated native team must never start a second Node transport.
  if [ -f "$CONNECTION_ROOT/run/native-sync/$team/engine" ]; then
    echo 'Native sync is selected. Use agent-mail sync start TEAM after this command.' >&2
    return 1
  fi
"#;

pub(super) const ENVIRONMENT: &[&str] = &[
    "AGMSG_CONFIG",
    "AGMSG_STORAGE_PATH",
    "AGMSG_STORAGE_DRIVER",
    "AGMSG_BUSY_TIMEOUT",
];
fn environment() -> Result<BTreeMap<String, String>> {
    let mut values: BTreeMap<String, String> = ENVIRONMENT
        .iter()
        .filter_map(|name| env::var(name).ok().map(|value| ((*name).into(), value)))
        .collect();
    for name in ["AGMSG_CONFIG", "AGMSG_STORAGE_PATH"] {
        if let Some(value) = values.get_mut(name)
            && !value.is_empty()
        {
            let path = crate::store::native_path(Path::new(value));
            *value = if path.is_absolute() {
                path
            } else {
                env::current_dir().map_err(|e| e.to_string())?.join(path)
            }
            .to_str()
            .ok_or("storage configuration path is not UTF-8")?
            .to_string();
        }
    }
    Ok(values)
}
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(super) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())
                .map_err(|e| e.to_string())?;
        }
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
#[derive(Deserialize, Serialize)]
struct Entry {
    path: String,
    digest: String,
}
#[derive(Deserialize, Serialize)]
struct Manifest {
    install_id: String,
    r#gen: u64,
    version: String,
    bootstrap_version: u64,
    created_at: String,
    digest_algo: String,
    files: Vec<Entry>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}
fn inventory(path: &Path, relative: &Path, paths: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let rel = relative.join(entry.file_name());
        if kind.is_dir() {
            inventory(&entry.path(), &rel, paths)?;
        } else if kind.is_file() {
            paths.insert(
                rel.to_str()
                    .ok_or("non-UTF8 script path")?
                    .replace('\\', "/"),
            );
        } else {
            return Err("refusing symlink or special file under agmsg scripts".into());
        }
    }
    Ok(())
}
fn patched(original: &[u8]) -> Result<Vec<u8>> {
    if hash(original) != ORIGINAL {
        return Err("unsupported agmsg sync helper; expected pinned v1.7.0".into());
    }
    let text = std::str::from_utf8(original).map_err(|e| e.to_string())?;
    if text.matches(CALL).count() != 1 || text.matches(LOW_LEVEL).count() != 1 {
        return Err("upstream sync hook call is ambiguous".into());
    }
    Ok(text
        .replacen(CALL, REPLACEMENT, 1)
        .replacen(LOW_LEVEL, GUARDED, 1)
        .into_bytes())
}

/// A crashed publication can be repaired only from the exact pinned backup.
/// Other modifications (including an upstream upgrade) require explicit review.
fn install(root: &Path) -> Result<bool> {
    if fs::read_to_string(root.join("VERSION"))
        .map_err(|e| e.to_string())?
        .trim()
        .trim_start_matches('v')
        != "1.7.0"
    {
        return Err("native hook integration requires agmsg v1.7.0".into());
    }
    let run = root.join("run");
    runtime::prepare_directory(&run)?;
    let mut fence =
        rusqlite::Connection::open(run.join("install-op.lock.db")).map_err(|e| e.to_string())?;
    fence
        .busy_timeout(Duration::from_secs(30))
        .map_err(|e| e.to_string())?;
    let transaction = fence
        .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
        .map_err(|e| e.to_string())?;
    let current = run.join("install-manifest.json");
    let previous = run.join("install-manifest.json.prev");
    let recovering = !current.exists();
    let bytes = fs::read(if recovering { &previous } else { &current }).map_err(
        |_| "missing official install manifest; finish the official agmsg installation first",
    )?;
    let mut manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid install manifest: {e}"))?;
    if manifest.version != "1.7.0" && manifest.version != "v1.7.0" {
        return Err("unsupported install manifest version".into());
    }
    if manifest.digest_algo != "sha256"
        || manifest.r#gen == 0
        || manifest.bootstrap_version != 1
        || manifest.install_id.is_empty()
    {
        return Err("unsupported install manifest contract".into());
    }
    let source = fs::read(root.join(HELPER)).map_err(|e| e.to_string())?;
    let backup = run.join("agent-mail-native-bridge/remote.original.sh");
    let original = if hash(&source) == ORIGINAL {
        source.clone()
    } else {
        fs::read(&backup)
            .map_err(|_| "modified sync helper without a pinned backup; refusing to overwrite")?
    };
    let replacement = patched(&original)?;
    if source != original && source != replacement {
        return Err("sync helper differs from both upstream and our known patch".into());
    }
    let mut listed = BTreeSet::new();
    for entry in &manifest.files {
        let path = Path::new(&entry.path);
        if !entry.path.starts_with("scripts/")
            || entry.path.contains('\\')
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || !listed.insert(entry.path.clone())
        {
            return Err("unsafe or duplicate install manifest path".into());
        }
        let actual = fs::read(root.join(path)).map_err(|e| e.to_string())?;
        if hash(&actual) != entry.digest
            && !(recovering
                && entry.path == HELPER
                && entry.digest == ORIGINAL
                && actual == replacement)
        {
            return Err(format!("agmsg integrity mismatch: {}", entry.path));
        }
    }
    let mut actual = BTreeSet::new();
    inventory(&root.join("scripts"), Path::new("scripts"), &mut actual)?;
    if actual != listed || !listed.contains(HELPER) {
        return Err("agmsg manifest inventory does not match installed scripts".into());
    }
    if source == replacement && !recovering {
        return Ok(false);
    }
    runtime::prepare_directory(backup.parent().ok_or("missing backup directory")?)?;
    atomic(&backup, &original)?;
    manifest.r#gen = manifest
        .r#gen
        .checked_add(1)
        .ok_or("install generation exhausted")?;
    manifest.created_at = transaction
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    manifest
        .files
        .iter_mut()
        .find(|e| e.path == HELPER)
        .ok_or("missing helper entry")?
        .digest = hash(&replacement);
    let next = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    if !recovering {
        fs::rename(&current, &previous).map_err(|e| e.to_string())?;
    }
    atomic(&root.join(HELPER), &replacement)?;
    atomic(&current, &next)?;
    transaction.commit().map_err(|e| e.to_string())?;
    Ok(true)
}

#[derive(Clone)]
struct Daemon {
    operation: u64,
    owner: u64,
    state: String,
}
fn daemon(root: &Path) -> Result<Option<Daemon>> {
    let path = root.join("run/install.db");
    if !path.exists() {
        return Ok(None);
    }
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let (desired, operation, owner, state): (Option<String>, u64, u64, String) = db.query_row(
        "SELECT daemon_intent.desired, daemon_intent.op_gen, daemon_owner.gen, daemon_owner.state FROM daemon_intent CROSS JOIN daemon_owner", [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(|e| format!("cannot read agmsgd intent: {e}"))?;
    Ok((desired.as_deref() == Some("on")).then_some(Daemon {
        operation,
        owner,
        state,
    }))
}
async fn resume_daemon(
    agmsg: &crate::agmsg::Agmsg,
    before: Option<Daemon>,
    changed: bool,
) -> Result<()> {
    let Some(before) = before else {
        return Ok(());
    };
    if changed && matches!(before.state.as_str(), "ready" | "starting") {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let Some(now) = daemon(agmsg.root())? else {
                return Ok(());
            };
            // Never undo an explicit daemon stop/start by another caller.
            if now.operation != before.operation {
                return Ok(());
            }
            if now.owner != before.owner || !matches!(now.state.as_str(), "ready" | "starting") {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("agmsgd did not step aside after the bridge update; inspect agmsg daemon status before retrying sync enable".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    agmsg.resume_daemon(before.operation).await
}

pub async fn enable(agmsg: &crate::agmsg::Agmsg, team: &str, no_login: bool) -> Result<()> {
    let root = agmsg.root();
    let control = runtime::ControlLock::acquire(root, team)?;
    // Validate storage/binding before stopping an existing working engine.
    drop(Context::open_for_adoption(root, team, true)?);
    let before = daemon(root)?;
    let changed = install(root)?;
    resume_daemon(agmsg, before, changed).await?;
    let dir = runtime::directory(root, team)?;
    runtime::prepare_directory(&dir)?;
    let exe = crate::agmsg::shell_path(&env::current_exe().map_err(|e| e.to_string())?);
    let exe = exe.to_str().ok_or("executable path is not UTF-8")?;
    if exe.contains(['\n', '\r']) {
        return Err("executable path contains a newline".into());
    }
    atomic(
        &dir.join("environment.json"),
        &serde_json::to_vec(&environment()?).map_err(|e| e.to_string())?,
    )?;
    atomic(&dir.join("engine"), format!("{exe}\n").as_bytes())?;
    agmsg.stop_remote(team).await?;
    service::remove(root, team)?;
    runtime::stop(root, team).await?;
    if !no_login {
        service::install(root, team)?;
    }
    drop(control);
    runtime::start(root, team)?;
    Ok(())
}
pub async fn disable(root: &Path, team: &str) -> Result<()> {
    let _control = runtime::ControlLock::acquire(root, team)?;
    service::remove(root, team)?;
    runtime::stop(root, team).await?;
    let marker = runtime::directory(root, team)?.join("engine");
    if marker.exists() {
        fs::remove_file(marker).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_sources_cannot_be_adopted() {
        assert!(patched(b"#!/bin/bash\n").is_err());
    }
}
