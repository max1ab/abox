//! Native registration; remote import and session ownership still use the upstream bridge.
use std::{env, ffi::OsString, path::Path};

use crate::{
    agmsg::{Agmsg, shell_path},
    cli::AgentType,
};

pub fn import(agmsg: &Agmsg, team: &str, server: &str) -> Result<(), String> {
    crate::store::team_name(team)?;
    let url = reqwest::Url::parse(server).map_err(|e| format!("invalid server URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("server must be an HTTP or HTTPS URL".into());
    }
    if team_exists(agmsg, team)? {
        return Err("team already exists locally; use join to register an identity, or upstream remote tools to repair its connection".into());
    }
    print!(
        "{}",
        agmsg.capture(
            "remote.sh",
            &[
                "pull".into(),
                "--endpoint".into(),
                server.into(),
                team.into(),
            ],
            &[]
        )?
    );
    if !team_exists(agmsg, team)? {
        return Err("remote import returned without creating the local team; inspect upstream remote status".into());
    }
    Ok(())
}

fn team_exists(agmsg: &Agmsg, team: &str) -> Result<bool, String> {
    crate::store::team_name(team)?;
    Ok(agmsg
        .root()
        .join("teams")
        .join(team)
        .join("config.json")
        .is_file())
}

pub fn join(
    agmsg: &Agmsg,
    team: &str,
    name: &str,
    kind: AgentType,
    project: &Path,
    create: bool,
) -> Result<(), String> {
    crate::registry::join(agmsg.root(), team, name, kind, project, create)
}

pub fn bind(
    agmsg: &Agmsg,
    team: &str,
    name: &str,
    kind: AgentType,
    project: &Path,
    session_id: Option<String>,
) -> Result<(), String> {
    crate::store::team_name(team)?;
    crate::store::team_name(name)?;
    let project = project
        .canonicalize()
        .map_err(|e| format!("cannot open project: {e}"))?;
    let variable = match kind {
        AgentType::Codex => "CODEX_THREAD_ID",
        AgentType::ClaudeCode => "CLAUDE_CODE_SESSION_ID",
    };
    let session = session_id.or_else(|| env::var(variable).ok())
        .ok_or_else(|| format!("current session is unknown; run inside the Agent or provide --session-id from its runtime ({variable})"))?;
    let parsed = uuid::Uuid::parse_str(&session).map_err(|_| "session ID must be a UUID")?;
    if session != parsed.to_string() {
        return Err("session ID must be a canonical UUID".into());
    }
    if !crate::registry::registered(
        &crate::registry::config(agmsg.root(), team)?,
        name,
        kind,
        &project,
    ) {
        return Err(
            "identity is not registered for this project and Agent type; run join first".into(),
        );
    }
    match kind {
        AgentType::Codex => {
            print!(
                "{}",
                agmsg.capture(
                    "drivers/types/codex/codex-record-session.sh",
                    &[team.into(), name.into(), shell_path(&project)],
                    &[(variable, &session)]
                )?
            );
        }
        AgentType::ClaudeCode => {
            // Upstream claims every matching team for this project/name.
            let output = agmsg.capture(
                "actas-claim.sh",
                &[
                    shell_path(&project),
                    kind.as_str().into(),
                    OsString::from(name),
                    session.clone().into(),
                ],
                &[],
            )?;
            print!("{output}");
            if !output
                .lines()
                .any(|line| line.split_whitespace().next() == Some("status=ok"))
            {
                return Err("upstream did not confirm the session claim".into());
            }
        }
    }
    if agmsg.recorded_session(team, name)? != session {
        return Err(
            "session binding was not recorded; check upstream session diagnostics before retrying"
                .into(),
        );
    }
    println!(
        "Bound {team}/{name} to {session}. Notification delivery is configured separately through agmsg."
    );
    Ok(())
}
