//! A single notification destination per (team, identity), compatible with
//! agmsgd's role-session reader. Binding does not install agent hooks.
use crate::{cli::AgentType, registry, store::TeamLock};
use std::{collections::BTreeMap, env, fs, path::Path};

type Result<T> = std::result::Result<T, String>;
pub(crate) fn encode(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub(crate) fn read(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.to_string()),
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
            return Err("session record is not a regular file".into());
        }
        Ok(_) => fs::read_to_string(path).map_err(|e| e.to_string())?,
    };
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key.to_owned()).or_insert(value.to_owned());
        }
    }
    Ok(fields)
}
fn absolute(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("profile/socket path contains an invalid character".into());
    }
    let path = crate::store::native_path(Path::new(raw));
    if !path.is_absolute() {
        return Err("profile/socket path must be absolute".into());
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "path is not UTF-8".into())
}
fn owned_session(owner: &str, session: &str) -> bool {
    owner == session
        || owner.strip_prefix(session).is_some_and(|rest| {
            rest.starts_with('.') && rest[1..].parse::<u32>().is_ok_and(|pid| pid > 1)
        })
}

pub fn bind(
    root: &Path,
    team: &str,
    name: &str,
    kind: AgentType,
    project: &Path,
    supplied: Option<String>,
    replace: bool,
) -> Result<()> {
    crate::store::team_name(team)?;
    crate::store::team_name(name)?;
    let project = project
        .canonicalize()
        .map_err(|e| format!("cannot open project: {e}"))?;
    if !project.is_dir() || project.to_string_lossy().chars().any(char::is_control) {
        return Err("project must be a directory with a valid path".into());
    }
    let variable = match kind {
        AgentType::Codex => "CODEX_THREAD_ID",
        AgentType::ClaudeCode => "CLAUDE_CODE_SESSION_ID",
    };
    let runtime = env::var(variable).ok();
    let session=supplied.or_else(||runtime.clone()).ok_or_else(||format!("current session is unknown; run inside the Agent or provide --session-id from its runtime ({variable})"))?;
    if !uuid::Uuid::parse_str(&session).is_ok_and(|id| id.to_string() == session) {
        return Err("session ID must be a canonical UUID".into());
    }
    if runtime.as_ref().is_some_and(|s| s != &session) {
        return Err("--session-id disagrees with the current runtime session".into());
    }
    let team_dir = root.join("teams").join(team);
    let _team = TeamLock::acquire(&team_dir)?;
    let config = registry::config(root, team)?;
    if config["import_pending"] == true {
        return Err("team import is incomplete; repeat team import before binding".into());
    }
    if !registry::registered(&config, name, kind, &project) {
        return Err(
            "identity is not registered for this project and Agent type; run join first".into(),
        );
    }
    let run = root.join("run");
    fs::create_dir_all(&run).map_err(|e| e.to_string())?;
    let _run = TeamLock::acquire(&run)?;
    let key = format!("{}__{}", encode(team), encode(name));
    let path = run.join(format!("role-session.{key}"));
    let old = read(&path)?;
    for (field, expected) in [("team", team), ("agent", name)] {
        if old.get(field).is_some_and(|v| v != expected) {
            return Err("session record belongs to a different identity; binding refused".into());
        }
    }
    if old.get("session").is_some_and(|s| s != &session) && !replace {
        return Err("identity is bound to another session; use --replace to explicitly change its receiving session".into());
    }
    // Existing Monitor/actas claims are a separate protocol. Do not steal them
    // or synthesize process-lifetime claims from a short-lived CLI process.
    let mut claims = vec![run.join(format!("actas.{key}.session"))];
    if let (Some(t), Some(m)) = (
        config["team_id"].as_str(),
        config["agents"][name]["member_id"].as_str(),
    ) {
        if !crate::sync::uuid(t, 7) || !crate::sync::uuid(m, 7) {
            return Err("invalid stable identity in team registry".into());
        }
        claims.push(run.join(format!("actas.{t}__{m}.session")));
    }
    let mut count = 0;
    for claim in claims {
        match fs::read_to_string(&claim) {
            Ok(owner) => {
                count += 1;
                if !owned_session(owner.trim(), &session) {
                    return Err("identity has a different upstream actas/Monitor owner; release that claim before binding".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot verify existing session ownership: {e}")),
        }
    }
    if count > 1 {
        return Err(
            "identity has conflicting upstream ownership files; repair them before binding".into(),
        );
    }
    let same =
        old.get("session") == Some(&session) && old.get("type").is_some_and(|t| t == kind.as_str());
    let mut fields = if same { old } else { BTreeMap::new() };
    for (key, value) in [
        ("session", session.as_str()),
        ("team", team),
        ("agent", name),
        ("type", kind.as_str()),
        ("project", project.to_str().ok_or("project is not UTF-8")?),
    ] {
        fields.insert(key.into(), value.into());
    }
    fields.insert("name".into(), format!("{team}-{name}"));
    fields.insert("updated_at".into(), registry::now()?);
    match kind {
        AgentType::Codex => {
            // An explicit profile takes precedence; repeated binding otherwise
            // preserves the profile previously published by the agent's hook.
            let home = if let Ok(raw) = env::var("CODEX_HOME") {
                absolute(&raw)?
            } else if let Some(home) = fields.get("codex_home") {
                absolute(home)?
            } else if let Ok(home) =
                fs::read_to_string(run.join(format!("codex-thread-home.{session}")))
            {
                absolute(home.lines().next().ok_or("empty Codex profile record")?)?
            } else {
                absolute(
                    crate::store::home()?
                        .join(".codex")
                        .to_str()
                        .ok_or("profile is not UTF-8")?,
                )?
            };
            fields.insert("codex_home".into(), home);
        }
        AgentType::ClaudeCode => {
            // Never attach the caller's socket or secret to an arbitrary UUID.
            if runtime.as_deref() == Some(&session)
                && let Ok(raw) = env::var("CLAUDE_CODE_MESSAGING_SOCKET")
            {
                let socket = absolute(raw.strip_prefix("uds:").unwrap_or(&raw))?;
                let profile = match env::var("CLAUDE_CONFIG_DIR") {
                    Ok(profile) => absolute(&profile)?,
                    Err(_) => match fields.get("claude_config_dir") {
                        Some(profile) => absolute(profile)?,
                        None => absolute(
                            crate::store::home()?
                                .join(".claude")
                                .to_str()
                                .ok_or("profile is not UTF-8")?,
                        )?,
                    },
                };
                if fields.get("messaging_socket") != Some(&socket)
                    || fields.get("claude_config_dir") != Some(&profile)
                {
                    fields.remove("messaging_token");
                }
                fields.insert("messaging_socket".into(), socket);
                fields.insert("claude_config_dir".into(), profile);
                fields.remove("messaging_blocked");
                if let Ok(token) = env::var("CLAUDE_CODE_MESSAGING_TOKEN") {
                    if !token
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                    {
                        return Err("invalid Claude messaging token".into());
                    }
                    if token.is_empty() {
                        fields.remove("messaging_token");
                    } else {
                        fields.insert("messaging_token".into(), token);
                    }
                }
            }
        }
    }
    let mut record = String::new();
    for (key, value) in &fields {
        if value.chars().any(char::is_control) {
            return Err("session metadata contains a control character".into());
        }
        record.push_str(&format!("{key}={value}\n"));
    }
    registry::atomic(&path, record.as_bytes())?;
    if read(&path)?.get("session") != Some(&session) {
        return Err("session binding was not recorded".into());
    }
    crate::daemon::wake(root);
    println!(
        "Bound {team}/{name} to {session}. Start agent-mail daemon and install the native hooks for ongoing delivery."
    );
    Ok(())
}
