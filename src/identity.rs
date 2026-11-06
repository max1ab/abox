//! Resolve the caller from runtime evidence and existing bindings, never from cwd alone.
use crate::{cli::AgentType, mailbox, registry, session};
use serde::Serialize;
use std::{env, path::Path};

type Result<T> = std::result::Result<T, String>;
#[derive(Serialize)]
pub struct Identity {
    pub team: String,
    pub name: String,
}

pub fn runtime(kind: Option<AgentType>, supplied: Option<String>) -> Result<(AgentType, String)> {
    let kind = kind
        .or_else(|| infer_type().ok())
        .ok_or("agent type is unknown or ambiguous; provide --type codex|claude-code")?;
    let key = match kind {
        AgentType::Codex => "CODEX_THREAD_ID",
        AgentType::ClaudeCode => "CLAUDE_CODE_SESSION_ID",
    };
    let current = env::var(key).ok().filter(|v| !v.is_empty());
    let id = supplied.or_else(|| current.clone()).ok_or(
        "current session is unknown; run inside the agent or provide --type and --session-id",
    )?;
    if current.as_ref().is_some_and(|v| v != &id) {
        return Err("session ID disagrees with runtime".into());
    }
    if !uuid::Uuid::parse_str(&id).is_ok_and(|u| u.to_string() == id) {
        return Err("session ID must be a canonical UUID".into());
    }
    Ok((kind, id))
}

pub fn infer_type() -> Result<AgentType> {
    let present = |key| env::var(key).is_ok_and(|s| !s.is_empty());
    match (
        present("CODEX_THREAD_ID"),
        present("CLAUDE_CODE_SESSION_ID"),
    ) {
        (true, false) => Ok(AgentType::Codex),
        (false, true) => Ok(AgentType::ClaudeCode),
        _ => Err("agent type is unknown or ambiguous; provide --type codex|claude-code".into()),
    }
}

pub fn bindings(root: &Path, kind: AgentType, id: &str) -> Result<Vec<Identity>> {
    let mut identities = vec![];
    for team in mailbox::teams(root)? {
        let team = team["name"].as_str().ok_or("invalid team name")?;
        let config = registry::config(root, team)?;
        let agents = config["agents"].as_object().ok_or("invalid team roster")?;
        for name in agents.keys() {
            let record = session::read(&root.join("run").join(format!(
                "role-session.{}__{}",
                session::encode(team),
                session::encode(name)
            )))?;
            if record.get("session").map(String::as_str) != Some(id)
                || record.get("type").map(String::as_str) != Some(kind.as_str())
            {
                continue;
            }
            if config["import_pending"] == true {
                return Err(format!("{team}: team import is incomplete"));
            }
            if record.get("team").map(String::as_str) != Some(team)
                || record.get("agent").map(String::as_str) != Some(name)
            {
                return Err("session identity mismatch".into());
            }
            let project = record.get("project").ok_or("session project missing")?;
            let project = crate::store::native_path(Path::new(project))
                .canonicalize()
                .map_err(|_| "session project unavailable")?;
            if !registry::registered(&config, name, kind, &project) {
                return Err("session no longer matches local registration".into());
            }
            let (variable, field) = match kind {
                AgentType::Codex => ("CODEX_HOME", "codex_home"),
                AgentType::ClaudeCode => ("CLAUDE_CONFIG_DIR", "claude_config_dir"),
            };
            if let Ok(profile) = env::var(variable) {
                let path = |s: &str| {
                    let p = crate::store::native_path(Path::new(s));
                    p.canonicalize().unwrap_or(p)
                };
                if record.get(field).is_some_and(|v| path(v) != path(&profile)) {
                    continue;
                }
            }
            identities.push(Identity {
                team: team.into(),
                name: name.clone(),
            });
        }
    }
    Ok(identities)
}

pub fn resolve(root: &Path, team: Option<&str>, kind: AgentType, id: &str) -> Result<Identity> {
    let mut candidates = bindings(root, kind, id)?;
    candidates.retain(|v| team.is_none_or(|t| t == v.team));
    if candidates.len() != 1 {
        return Err(if candidates.is_empty() {
            "no identity is bound to this session; run agent-mail start --team TEAM --name NAME"
        } else {
            "this session has multiple identities; specify --team (each team must have one identity per session)"
        }.into());
    }
    Ok(candidates.remove(0))
}

pub fn team(
    root: &Path,
    explicit: Option<String>,
    kind: Option<AgentType>,
    id: Option<String>,
) -> Result<String> {
    if let Some(team) = explicit {
        crate::store::team_name(&team)?;
        registry::config(root, &team)?;
        return Ok(team);
    }
    let (kind, id) = runtime(kind, id)?;
    Ok(resolve(root, None, kind, &id)?.team)
}
