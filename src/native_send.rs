//! Native send: writes the event and its legacy mirror in one transaction.
use rusqlite::{TransactionBehavior, params};
use serde_json::Value;
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

pub fn send(
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
    let mut store = crate::store::open(root, team)?;
    let cfg = &store.config;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_nul_before_opening_any_mailbox() {
        let temp = tempfile::tempdir().unwrap();
        let body = temp.path().join("body.txt");
        std::fs::write(&body, b"hello\0world").unwrap();
        let error =
            super::send(&temp.path().join("absent"), "dev", "a", "b", Some(&body)).unwrap_err();
        assert!(error.contains("NUL"));
        assert!(!temp.path().join("absent").exists());
    }
}
