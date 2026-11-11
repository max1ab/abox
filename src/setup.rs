//! Setup commands implemented natively against the local mailbox and relay.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use crate::{agmsg::Agmsg, cli::AgentType};

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Defaults {
    pub team: Option<String>,
    pub machine: Option<String>,
    pub server: Option<String>,
}

impl Defaults {
    pub(crate) fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("client.json");
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| format!("invalid client settings in {}: {e}", path.display())),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!(
                "cannot read client settings in {}: {e}",
                path.display()
            )),
        }
    }

    pub(crate) fn validated(
        team: Option<String>,
        machine: Option<String>,
        server: Option<&str>,
    ) -> Result<Self, String> {
        if let Some(team) = &team {
            crate::store::team_name(team)?;
            if team.chars().count() > 128 {
                return Err("default team name must be at most 128 characters".into());
            }
        }
        if let Some(machine) = &machine {
            registration_name("a".into(), Some(machine))?;
        }
        let server = server
            .map(crate::sync::endpoint)
            .transpose()?
            .map(|url| url.to_string());
        Ok(Self {
            team,
            machine,
            server,
        })
    }

    pub(crate) fn save(self, root: &Path) -> Result<(), String> {
        if self.team.is_none() && self.machine.is_none() && self.server.is_none() {
            return Ok(());
        }
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".agent-mail-init.lock"))
            .map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        let old = Self::load(root)?;
        let settings = Self {
            team: self.team.or(old.team),
            machine: self.machine.or(old.machine),
            server: self.server.or(old.server),
        };
        crate::registry::atomic(
            &root.join("client.json"),
            &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
        )
    }
}

pub(crate) fn relay_address(
    root: Option<PathBuf>,
    fallback: String,
    configured: bool,
) -> Result<String, String> {
    if configured {
        return Ok(fallback);
    }
    Ok(Defaults::load(&Agmsg::location(root)?)?
        .server
        .unwrap_or(fallback))
}

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
