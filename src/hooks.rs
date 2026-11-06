//! Agent-native lifecycle hooks. The host invokes our binary; no Bash wrapper,
//! Node helper, jq, or external sqlite3 is installed.
use crate::{cli::AgentType, registry, session};
use serde_json::{Value, json};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, String>;
fn quote(value: &str, windows: bool) -> String {
    if windows {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}
fn commands(root: &Path, kind: AgentType) -> Result<(String, String)> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let values = [
        exe.to_str().ok_or("executable is not UTF-8")?,
        "--data-dir",
        root.to_str().ok_or("data path is not UTF-8")?,
        "hook",
        "run",
        "--type",
        kind.as_str(),
    ];
    if values.iter().any(|s| s.chars().any(char::is_control)) {
        return Err("hook paths contain control characters".into());
    }
    let posix = values
        .iter()
        .map(|s| quote(s, false))
        .collect::<Vec<_>>()
        .join(" ");
    let windows = format!(
        "& {}",
        values
            .iter()
            .map(|s| quote(s, true))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok((posix, windows))
}
fn default_config(kind: AgentType) -> Result<PathBuf> {
    let profile = match kind {
        AgentType::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or(crate::store::home()?.join(".codex")),
        AgentType::ClaudeCode => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or(crate::store::home()?.join(".claude")),
    };
    if !profile.is_absolute() {
        return Err("agent profile must be absolute".into());
    }
    Ok(profile.join(if kind.as_str() == "codex" {
        "hooks.json"
    } else {
        "settings.json"
    }))
}
fn owns_legacy(command: &str, root: &Path) -> bool {
    let normalize = |s: &str| {
        let s = s
            .replace("'\"'\"'", "'")
            .replace("''", "'")
            .replace("\\\\", "\\")
            .replace('\\', "/");
        let s = s.strip_prefix("//?/").unwrap_or(&s).to_owned();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    let scripts = root.join("scripts");
    if normalize(command).contains(&format!("{}/", normalize(&scripts.to_string_lossy()))) {
        return true;
    }
    // Resolve aliases of the installation root (e.g. /var versus /private/var)
    // without executing or expanding any part of the command.
    let mut pending = vec![command.to_owned()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for value in pending {
            if let Some(words) = shlex::split(&value) {
                for word in words.into_iter().take(256) {
                    let path = crate::store::native_path(Path::new(&word));
                    if path.is_absolute() {
                        let mut existing = path;
                        let mut tail = Vec::new();
                        loop {
                            if let Ok(mut resolved) = existing.canonicalize() {
                                for part in tail.iter().rev() {
                                    resolved.push(part);
                                }
                                if resolved.starts_with(&scripts) {
                                    return true;
                                }
                                break;
                            }
                            let Some(part) = existing.file_name().map(|s| s.to_owned()) else {
                                break;
                            };
                            tail.push(part);
                            if !existing.pop() {
                                break;
                            }
                        }
                    }
                    if word != value {
                        next.push(word);
                    }
                }
            }
        }
        pending = next;
    }
    false
}

fn prune(config: &mut Value, owned: &[String]) -> Result<()> {
    let Some(events) = config.get_mut("hooks") else {
        return Ok(());
    };
    let events = events.as_object_mut().ok_or("hooks must be an object")?;
    for groups in events.values_mut() {
        let groups = groups
            .as_array_mut()
            .ok_or("hook groups must be an array")?;
        for group in groups.iter_mut() {
            if let Some(handlers) = group.get_mut("hooks") {
                let handlers = handlers
                    .as_array_mut()
                    .ok_or("hook handlers must be an array")?;
                handlers.retain(|h| {
                    !owned
                        .iter()
                        .any(|cmd| h["command"] == *cmd || h["commandWindows"] == *cmd)
                });
            }
        }
        groups.retain(|g| g["hooks"].as_array().is_none_or(|h| !h.is_empty()));
    }
    events.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    Ok(())
}
pub(crate) fn install(
    root: &Path,
    kind: AgentType,
    path: Option<PathBuf>,
    replace_agmsg: bool,
    remove: bool,
) -> Result<()> {
    let path = match path {
        Some(path) => path,
        None => default_config(kind)?,
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let parent = path.parent().ok_or("hook config has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let _lock = crate::store::TeamLock::acquire(parent)?;
    let path = parent
        .canonicalize()
        .map_err(|e| e.to_string())?
        .join(path.file_name().ok_or("missing config name")?);
    if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err("refusing a symlink hook configuration".into());
    }
    if kind.as_str() == "codex" && !remove {
        let inline = parent.join("config.toml");
        match fs::read_to_string(&inline) {
            Ok(text) if owns_legacy(&text,root)=>return Err("old agmsg hooks are present in config.toml; remove those inline hook entries before installing native hooks.json".into()),
            Ok(_)=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
        }
    }
    let before = match fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let mut config = match &before {
        Some(b) => serde_json::from_slice::<Value>(b)
            .map_err(|_| "hook configuration is not valid JSON")?,
        None => json!({}),
    };
    if !config.is_object() {
        return Err("hook configuration must be an object".into());
    }
    let hash: String =
        ring::digest::digest(&ring::digest::SHA256, path.to_string_lossy().as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
    let manifest = root
        .join("run")
        .join(format!("hooks-{}-{hash}.json", kind.as_str()));
    let old = match fs::read(&manifest) {
        Ok(b) => serde_json::from_slice::<Vec<String>>(&b)
            .map_err(|_| "invalid installed-hook record")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(e) => return Err(e.to_string()),
    };
    let (posix, windows) = commands(root, kind)?;
    let mut owned = old;
    owned.push(posix.clone());
    owned.push(windows.clone());
    if let Some(events) = config["hooks"].as_object() {
        for groups in events.values() {
            for group in groups.as_array().ok_or("hook groups must be an array")? {
                for handler in group["hooks"]
                    .as_array()
                    .ok_or("hook handlers must be an array")?
                {
                    for field in ["command", "commandWindows"] {
                        if let Some(command) = handler[field].as_str()
                            && owns_legacy(command, root)
                        {
                            if !replace_agmsg && !remove {
                                return Err("existing agmsg hooks use this mailbox; use --replace-agmsg to replace only those handlers (inline config.toml hooks must be removed separately)".into());
                            }
                            if replace_agmsg {
                                owned.push(command.into());
                            }
                        }
                    }
                }
            }
        }
    }
    prune(&mut config, &owned)?;
    if !remove {
        if config.get("hooks").is_none() {
            config["hooks"] = json!({});
        }
        for event in ["SessionStart", "UserPromptSubmit"] {
            if config["hooks"].get(event).is_none() {
                config["hooks"][event] = json!([]);
            }
            let mut handler = json!({"type":"command","command":posix,"timeout":10});
            if kind.as_str() == "codex" {
                handler["commandWindows"] = json!(windows);
            } else if cfg!(windows) {
                handler["command"] = json!(windows);
                handler["shell"] = json!("powershell");
            }
            config["hooks"][event]
                .as_array_mut()
                .ok_or("hook groups must be arrays")?
                .push(json!({"matcher":"","hooks":[handler]}));
        }
    }
    let updated = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
    if before.as_deref() != Some(updated.as_slice()) {
        if let Some(bytes) = before {
            let backup = path.with_extension(format!("agent-mail-backup-{}", uuid::Uuid::new_v4()));
            registry::atomic(&backup, &bytes)?;
        }
        registry::atomic(&path, &updated)?;
    }
    fs::create_dir_all(root.join("run")).map_err(|e| e.to_string())?;
    registry::atomic(
        &manifest,
        serde_json::to_vec(&if remove {
            Vec::<String>::new()
        } else {
            vec![posix, windows]
        })
        .map_err(|e| e.to_string())?
        .as_slice(),
    )?;
    eprintln!(
        "{} native hooks in {}. Reload hooks in the agent; review new Codex hooks through /hooks.",
        if remove { "Removed" } else { "Installed" },
        path.display()
    );
    Ok(())
}
pub(crate) fn run(root: &Path, kind: AgentType) -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(262145)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 262144 {
        return Err("hook input exceeds 256 KiB".into());
    }
    let input: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid hook JSON")?;
    let event = input["hook_event_name"]
        .as_str()
        .ok_or("hook event missing")?;
    if !matches!(event, "SessionStart" | "UserPromptSubmit") {
        return Err("unsupported lifecycle hook".into());
    }
    let id = input["session_id"]
        .as_str()
        .ok_or("hook session ID missing")?;
    if !uuid::Uuid::parse_str(id).is_ok_and(|u| u.to_string() == id) {
        return Err("hook session ID is not a UUID".into());
    }
    let cwd =
        crate::store::native_path(Path::new(input["cwd"].as_str().ok_or("hook cwd missing")?))
            .canonicalize()
            .map_err(|_| "hook project unavailable")?;
    let socket = std::env::var("CLAUDE_CODE_MESSAGING_SOCKET")
        .ok()
        .map(|s| s.strip_prefix("uds:").unwrap_or(&s).to_owned());
    let mut bound = Vec::new();
    for team in crate::mailbox::teams(root)? {
        let team = team["name"].as_str().ok_or("invalid team name")?;
        let cfg = registry::config(root, team)?;
        if cfg["import_pending"] == true {
            continue;
        }
        for (name, _) in cfg["agents"].as_object().ok_or("invalid roster")? {
            let path = root.join("run").join(format!(
                "role-session.{}__{}",
                session::encode(team),
                session::encode(name)
            ));
            let record = session::read(&path)?;
            let Some(project) = record
                .get("project")
                .and_then(|p| crate::store::native_path(Path::new(p)).canonicalize().ok())
            else {
                continue;
            };
            if !cwd.starts_with(&project) || !registry::registered(&cfg, name, kind, &project) {
                continue;
            }
            let same = record.get("session").map(String::as_str) == Some(id);
            let clear = kind.as_str() == "claude-code"
                && socket
                    .as_ref()
                    .is_some_and(|s| record.get("messaging_socket") == Some(s));
            // Never adopt a different live session just because it shares cwd.
            if !same && !clear {
                continue;
            }
            let mut cmd = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
            cmd.arg("--data-dir")
                .arg(root)
                .args([
                    "bind",
                    team,
                    "--name",
                    name,
                    "--type",
                    kind.as_str(),
                    "--session-id",
                    id,
                    "--project",
                ])
                .arg(&project)
                .env(
                    if kind.as_str() == "codex" {
                        "CODEX_THREAD_ID"
                    } else {
                        "CLAUDE_CODE_SESSION_ID"
                    },
                    id,
                );
            if clear && !same {
                cmd.arg("--replace");
            }
            let output = cmd
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|_| "cannot refresh session binding")?;
            if !output.status.success() {
                return Err("session binding refresh failed; inspect bind diagnostics".into());
            }
            bound.push(format!("{team}/{name}"));
        }
    }
    crate::daemon::wake(root);
    if !bound.is_empty() {
        println!(
            "{}",
            json!({"hookSpecificOutput":{"hookEventName":event,"additionalContext":format!("Agent Mail identities for this session: {}. On [agent-mail:nonce] notices, run agent-mail inbox to read mail and acknowledge the pending reminder.",bound.join(", "))}})
        );
    }
    Ok(())
}
