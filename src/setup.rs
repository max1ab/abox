//! Setup commands implemented natively against the local mailbox and relay.
use std::path::Path;

use crate::{agmsg::Agmsg, cli::AgentType};

pub async fn import(agmsg: &Agmsg, team: &str, server: &str) -> Result<(), String> {
    crate::import::run(agmsg.root(), team, server).await
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
