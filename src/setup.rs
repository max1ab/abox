//! Setup commands implemented natively against the local mailbox and relay.
use std::path::Path;

use crate::{agmsg::Agmsg, cli::AgentType};

pub fn registration_name(name: String, machine: Option<&str>) -> Result<String, String> {
    let Some(machine) = machine else {
        return Ok(name);
    };
    crate::store::team_name(&name)?;
    crate::store::team_name(machine)?;
    if name.contains('@') || machine.contains('@') {
        return Err("provide the agent name and machine label separately, without '@'".into());
    }
    let address = format!("{name}@{machine}");
    if address.chars().count() > 128 {
        return Err("the qualified agent name must be at most 128 characters".into());
    }
    Ok(address)
}

pub async fn import(agmsg: &Agmsg, team: &str, server: &str) -> Result<(), String> {
    crate::import::run(agmsg.root(), team, server, false).await?;
    println!(
        "Imported team {team}. Run agent-mail start --team {team} --name NAME from the receiving session."
    );
    Ok(())
}

pub async fn import_for_start(
    agmsg: &Agmsg,
    team: &str,
    server: &str,
    explicit: bool,
) -> Result<(), String> {
    crate::store::team_name(team)?;
    let root = agmsg.root();
    let path = root.join("teams").join(team).join("config.json");
    if path.try_exists().map_err(|e| e.to_string())? {
        let config = crate::registry::config(root, team)?;
        if config["attachment_pending"] != true && config["import_pending"] != true {
            if !explicit {
                return Ok(());
            }
            let binding = &config["remote_binding"];
            if binding.is_object() {
                if binding["endpoint"] == crate::sync::endpoint(server)?.as_str() {
                    return Ok(());
                }
                return Err("team is already bound to a different relay".into());
            }
        }
    }
    crate::import::run(root, team, server, explicit).await
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
    replace: bool,
) -> Result<(), String> {
    crate::session::bind(agmsg.root(), team, name, kind, project, session_id, replace)
}
