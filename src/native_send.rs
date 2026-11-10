//! Native send: writes the event and its legacy mirror in one transaction.
use rusqlite::{TransactionBehavior, params};
use serde_json::Value;
use std::{
    fs,
    io::{self, Read},
    path::Path,
    time::Duration,
};

pub async fn send(
    root: &Path,
    team: &str,
    from: &str,
    to: &str,
    body_file: Option<&Path>,
) -> Result<(), String> {
    let mut body = String::new();
    if let Some(path) = body_file {
        body = fs::read_to_string(path)
            .map_err(|e| format!("cannot read UTF-8 body file {}: {e}", path.display()))?;
    } else {
        io::stdin()
            .read_to_string(&mut body)
            .map_err(|e| format!("cannot read UTF-8 stdin: {e}"))?;
    }
    // send.sh's command substitution strips LF only, not CR or other whitespace.
    let body = body.trim_end_matches('\n');
    if body.is_empty() {
        return Err("the message body is empty — nothing to send".into());
    }
    if body.contains('\0') {
        return Err("message body contains NUL; refusing lossy shell-compatible delivery".into());
    }
    let store = crate::store::open(root, team)?;
    let relay = recipient_relay(&store.config, team, from, to)?.cloned();
    let mut store = if let Some(binding) = &relay {
        let recipient = store.config["agents"][to].clone();
        // The relay check must not hold the registry lock or a SQLite connection.
        drop(store);
        check_relay(binding).await.map_err(|error| {
            format!("cannot send to remote agent '{to}' in team '{team}': {error}. Retry when the relay is available")
        })?;
        let store = crate::store::open(root, team)?;
        if recipient_relay(&store.config, team, from, to)? != Some(binding)
            || store.config["agents"][to] != recipient
        {
            return Err("recipient route changed during send. Retry the command".into());
        }
        store
    } else {
        store
    };
    let tx = store
        .db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| format!("cannot start send transaction: {e}"))?;
    crate::store::validate(&tx)?;
    let id = uuid::Uuid::now_v7().to_string();
    let at: String = tx
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO messages(team,from_agent,to_agent,body,created_at) VALUES (?1,?2,?3,?4,?5)",
        params![team, from, to, body, at],
    )
    .map_err(|e| format!("cannot write message: {e}"))?;
    let legacy_id = tx.last_insert_rowid();
    tx.execute("INSERT INTO events(type,id,team,from_agent,to_agent,body,at,legacy_id) VALUES ('message_sent',?1,?2,?3,?4,?5,?6,?7)", params![id,team,from,to,body,at,legacy_id])
        .map_err(|e| format!("cannot write message event (send rolled back): {e}"))?;
    tx.commit()
        .map_err(|e| format!("cannot commit send: {e}"))?;
    drop(store);
    // Notification is a latency hint. The committed outbox remains durable
    // even when no resident process is running or the datagram is dropped.
    crate::sync::runtime::notify(root, team);
    if relay.is_some() {
        println!("Queued for remote delivery to {to} in team {team}");
    } else {
        println!("Sent to {to} in team {team}");
    }
    Ok(())
}

fn recipient_relay<'a>(
    cfg: &'a Value,
    team: &str,
    from: &str,
    to: &str,
) -> Result<Option<&'a Value>, String> {
    if cfg["import_pending"] == true {
        return Err("team import is incomplete; repeat team import before sending".into());
    }
    let agents = cfg
        .get("agents")
        .and_then(Value::as_object)
        .ok_or("invalid team roster")?;
    for (role, name) in [("from", from), ("to", to)] {
        let member = agents
            .get(name)
            .filter(|x| !x.is_null())
            .ok_or_else(|| format!("{role} agent '{name}' is not registered in team '{team}'"))?;
        // Other types may dispatch _message.sh after commit. Do not silently
        // skip those hooks, nor invoke a shell from the native path.
        let mut types = Vec::new();
        if let Some(t) = member.get("type").and_then(Value::as_str) {
            types.push(t);
        }
        if let Some(regs) = member.get("registrations").and_then(Value::as_array) {
            types.extend(
                regs.iter()
                    .filter_map(|r| r.get("type").and_then(Value::as_str)),
            );
        }
        if role == "to" && types.iter().any(|t| !matches!(*t, "codex" | "claude-code")) {
            return Err("native send currently supports Codex/Claude recipients only (no extension dispatch)".into());
        }
    }
    let recipient = &agents[to];
    let local = recipient["registrations"].as_array().map_or_else(
        || recipient["type"].is_string(),
        |registrations| !registrations.is_empty(),
    );
    if local {
        return Ok(None);
    }
    let binding = &cfg["remote_binding"];
    if !binding.is_object() || binding.get("disconnected_at") != Some(&Value::Null) {
        return Err(format!(
            "remote agent '{to}' has no connected relay in team '{team}'"
        ));
    }
    if cfg["sync_enabled"] == false {
        return Err(format!("remote delivery is paused for team '{team}'"));
    }
    Ok(Some(binding))
}

async fn check_relay(binding: &Value) -> Result<(), String> {
    if binding["protocol_version"] != 1 || binding["cipher_profile"] != "none" {
        return Err("remote delivery requires a protocol 1 plaintext binding".into());
    }
    let endpoint = binding["endpoint"]
        .as_str()
        .ok_or("missing relay endpoint")?;
    let server = binding["server_instance_id"]
        .as_str()
        .ok_or("missing relay ID")?;
    let team = binding["remote_team_id"]
        .as_str()
        .ok_or("missing remote team ID")?;
    if !crate::sync::uuid(server, 7) || !crate::sync::uuid(team, 7) {
        return Err("invalid connected server/team identity".into());
    }
    let url = crate::sync::endpoint(endpoint)?
        .join("v1/capabilities")
        .map_err(|_| "invalid relay route")?;
    let request = crate::remote::client()?
        .get(url)
        .timeout(Duration::from_secs(3));
    let caps = crate::remote::team_json(request, server, team).await?;
    crate::sync::plaintext_policy(&caps)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn rejects_nul_before_opening_any_mailbox() {
        let temp = tempfile::tempdir().unwrap();
        let body = temp.path().join("body.txt");
        std::fs::write(&body, b"hello\0world").unwrap();
        let error = super::send(&temp.path().join("absent"), "dev", "a", "b", Some(&body))
            .await
            .unwrap_err();
        assert!(error.contains("NUL"));
        assert!(!temp.path().join("absent").exists());
    }
}
