//! Native send: writes the event and its legacy mirror in one transaction.
use rusqlite::{TransactionBehavior, params};
use serde_json::Value;
use std::{
    fs,
    io::{self, Read},
    path::Path,
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
    let requested = to;
    let to = resolve_recipient(&store.config, requested)?.to_owned();
    let relay = recipient_relay(&store.config, team, from, &to)?.cloned();
    let mut store = if let Some(binding) = &relay {
        let recipient = store.config["agents"][&to].clone();
        // The relay check must not hold the registry lock or a SQLite connection.
        drop(store);
        check_recipient(binding, &recipient)
            .await
            .map_err(|error| {
                format!("cannot send to remote agent '{to}' in team '{team}': {error}")
            })?;
        let store = crate::store::open(root, team)?;
        if resolve_recipient(&store.config, requested)? != to
            || recipient_relay(&store.config, team, from, &to)? != Some(binding)
            || store.config["agents"][&to] != recipient
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
    crate::mailbox::ensure_addresses(&tx)?;
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
    crate::mailbox::record_addresses(
        &tx,
        tx.last_insert_rowid(),
        crate::presence::member_id(&store.config["agents"][from]),
        crate::presence::member_id(&store.config["agents"][&to]),
    )?;
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

fn resolve_recipient<'a>(config: &'a Value, requested: &'a str) -> Result<&'a str, String> {
    if requested.contains('@') {
        return Ok(requested);
    }
    let agents = config["agents"].as_object().ok_or("invalid team roster")?;
    let mut matches: Vec<_> = agents
        .iter()
        .filter(|(address, member)| {
            !member.is_null()
                && (*address == requested
                    || address
                        .rsplit_once('@')
                        .is_some_and(|(name, machine)| name == requested && !machine.is_empty()))
        })
        .map(|(address, _)| address.as_str())
        .collect();
    match matches.as_slice() {
        [] => Ok(requested),
        [address] => Ok(address),
        _ => {
            matches.sort_unstable();
            Err(format!(
                "agent name '{requested}' is ambiguous; use a full address: {}",
                matches.join(", ")
            ))
        }
    }
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
    if crate::presence::is_local(recipient) {
        return Ok(None);
    }
    if cfg["attachment_pending"] == true {
        return Err(
            "remote team attachment is incomplete; retry start with the relay address".into(),
        );
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

async fn check_recipient(binding: &Value, recipient: &Value) -> Result<(), String> {
    let member = crate::presence::member_id(recipient).ok_or("invalid remote member identity")?;
    let relay = crate::presence::Relay::from_binding(binding)?;
    let client = crate::remote::client()?;
    let capabilities = relay.request(&client, "v1/capabilities")?;
    let (caps, reachable) =
        tokio::try_join!(relay.json(capabilities), relay.query(&client, Some(member)))?;
    crate::sync::plaintext_policy(&caps)?;
    if !reachable.contains(member) {
        return Err("recipient is currently unreachable; message was not queued".into());
    }
    Ok(())
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
