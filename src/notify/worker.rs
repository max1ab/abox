use super::{State, Target, deliver};
use crate::{cli::AgentType, mailbox, registry, session};
use serde::Serialize;
use std::{collections::BTreeMap, fs, path::Path, time::Duration};
type Result<T> = std::result::Result<T, String>;
pub(crate) struct Group {
    pub target: Target,
    pub boxes: Vec<(String, String)>,
    pub unread: i64,
}
#[derive(Default, Serialize)]
pub(crate) struct Report {
    pub submitted: usize,
    pub errors: Vec<String>,
}
pub(crate) fn groups(root: &Path) -> Result<(BTreeMap<String, Group>, Vec<String>)> {
    groups_for(root, None)
}

fn bound_target(config: &serde_json::Value, team: &str, name: &str, path: &Path) -> Result<Target> {
    let record = session::read(path)?;
    if record.get("team").map(String::as_str) != Some(team)
        || record.get("agent").map(String::as_str) != Some(name)
    {
        return Err("session identity mismatch".into());
    }
    let target = Target::from_record(&record)?;
    let kind = match target.agent_type() {
        "codex" => AgentType::Codex,
        _ => AgentType::ClaudeCode,
    };
    let project = record.get("project").ok_or("session project missing")?;
    let project = crate::store::native_path(Path::new(project))
        .canonicalize()
        .map_err(|_| "session project unavailable")?;
    if !registry::registered(config, name, kind, &project) {
        return Err("session no longer matches local registration".into());
    }
    Ok(target)
}

pub(crate) fn bound_member_ids(
    root: &Path,
    team: &str,
    config: &serde_json::Value,
    requested: Option<&str>,
) -> Vec<String> {
    config["agents"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, member)| {
            let id = crate::presence::member_id(member)?;
            if requested.is_some_and(|requested| requested != id)
                || !crate::presence::is_local(member)
            {
                return None;
            }
            let path = binding_path(root, team, name);
            bound_target(config, team, name, &path)
                .ok()
                .map(|_| id.to_owned())
        })
        .collect()
}

fn binding_path(root: &Path, team: &str, name: &str) -> std::path::PathBuf {
    root.join("run").join(format!(
        "role-session.{}__{}",
        session::encode(team),
        session::encode(name)
    ))
}

fn groups_for(
    root: &Path,
    only: Option<(&str, &str)>,
) -> Result<(BTreeMap<String, Group>, Vec<String>)> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut errors = vec![];
    for team in mailbox::teams(root)? {
        let team = team["name"].as_str().ok_or("invalid team name")?;
        let config = match registry::config(root, team) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{team}: {e}"));
                continue;
            }
        };
        if config["import_pending"] == true {
            continue;
        }
        let Some(agents) = config["agents"].as_object() else {
            errors.push(format!("{team}: invalid roster"));
            continue;
        };
        for name in agents.keys() {
            let path = binding_path(root, team, name);
            if !path.try_exists().map_err(|e| e.to_string())? {
                continue;
            }
            if let Some((kind, id)) = only {
                let record = session::read(&path)?;
                if record.get("session").map(String::as_str) != Some(id)
                    || record.get("type").map(String::as_str) != Some(kind)
                {
                    continue;
                }
            }
            let read = || -> Result<(Target, i64)> {
                let target = bound_target(&config, team, name, &path)?;
                let count = match crate::store::open_existing(root, team)? {
                    Some(store) => mailbox::unread_count(
                        &store.db,
                        team,
                        name,
                        crate::presence::member_id(&store.config["agents"][name]),
                    )?,
                    None => 0,
                };
                Ok((target, count))
            };
            match read() {
                Ok((target, count)) => {
                    let group = groups.entry(target.key()).or_insert_with(|| Group {
                        target,
                        boxes: vec![],
                        unread: 0,
                    });
                    group.boxes.push((team.into(), name.clone()));
                    group.unread += count;
                }
                Err(e) => errors.push(format!("{team}/{name}: {e}")),
            }
        }
    }
    Ok((groups, errors))
}
pub(crate) async fn once(root: &Path) -> Result<Report> {
    crate::daemon::check_legacy(root)?;
    fs::create_dir_all(root.join("run")).map_err(|e| e.to_string())?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("run/notification-worker.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "another notification scan is running")?;
    let state = State::open(root)?;
    state.recover()?;
    let (groups, errors) = groups(root)?;
    let mut report = Report {
        submitted: 0,
        errors,
    };
    for (key, group) in groups {
        if group.unread == 0 {
            continue;
        }
        let Some(nonce) = state.reserve(&key)? else {
            continue;
        };
        let unchanged = group.boxes.iter().all(|(team, name)| {
            let path = root.join("run").join(format!(
                "role-session.{}__{}",
                session::encode(team),
                session::encode(name)
            ));
            session::read(&path)
                .and_then(|r| Target::from_record(&r))
                .is_ok_and(|target| target == group.target)
        });
        if !unchanged {
            state.finish(
                &nonce,
                super::Delivery::Failed {
                    reason: "session_binding_changed".into(),
                },
            )?;
            continue;
        }
        let message =
            format!("[agent-mail:{nonce}] New messages are waiting. Check your agent-mail inbox.");
        // No mailbox/registry locks are held across the external notification.
        let result = deliver(&group.target, &message, &nonce, Duration::from_secs(10)).await;
        match &result {
            super::Delivery::Accepted { .. } => report.submitted += 1,
            super::Delivery::Failed { reason } | super::Delivery::Unknown { reason } => report
                .errors
                .push(format!("{}: {reason}", group.target.session())),
        }
        state.finish(&nonce, result)?;
    }
    Ok(report)
}
pub(crate) fn inbox(
    root: &Path,
    kind: AgentType,
    id: Option<String>,
    nonce: Option<String>,
) -> Result<()> {
    let variable = if kind.as_str() == "codex" {
        "CODEX_THREAD_ID"
    } else {
        "CLAUDE_CODE_SESSION_ID"
    };
    let runtime = std::env::var(variable).ok();
    let id = id
        .or_else(|| runtime.clone())
        .ok_or("session is unknown; provide --session-id from the runtime")?;
    if runtime.is_some_and(|v| v != id) {
        return Err("session ID disagrees with runtime".into());
    }
    let (groups, errors) = groups_for(root, Some((kind.as_str(), &id)))?;
    // A broken binding can hide another mailbox, so do not acknowledge a
    // reminder while the complete receiving set cannot be established.
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let state = State::open(root)?;
    let expected = nonce.as_deref().map(|n| state.target(n)).transpose()?;
    let matches: Vec<_> = groups
        .into_iter()
        .filter(|(key, g)| {
            g.target.session() == id
                && g.target.agent_type() == kind.as_str()
                && expected.as_ref().is_none_or(|e| e == key)
        })
        .collect();
    let [(key, group)] = matches.as_slice() else {
        return Err("no unique bound receiving session; check bindings or use the reminder's --notification nonce".into());
    };
    // Capture before displaying any mail. A reminder created during this read
    // belongs to a later scan and must not be acknowledged by accident.
    let pending = match nonce {
        Some(nonce) => Some(nonce),
        None => state.pending(key)?,
    };
    for (team, agent) in &group.boxes {
        println!("{team}/{agent}:");
        mailbox::inbox(root, team, agent)?;
    }
    if let Some(nonce) = pending {
        state.acknowledge(&nonce, key)?;
    }
    Ok(())
}
