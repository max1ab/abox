//! Native registration of a local plaintext team. Stable UUIDs make a lost
//! registration response recoverable without minting a second remote team.
use crate::{registry, remote, store::TeamLock, sync};
use serde_json::{Value, json};
use std::path::Path;
type Result<T> = std::result::Result<T, String>;
pub(crate) async fn run(root: &Path, team: &str, server: &str) -> Result<()> {
    crate::store::team_name(team)?;
    let endpoint = sync::endpoint(server)?;
    let original = {
        let _lock = TeamLock::acquire(&root.join("teams").join(team))?;
        registry::config(root, team)?
    };
    if original["import_pending"] == true {
        return Err("finish team import before connecting".into());
    }
    if original["attachment_pending"] == true
        || original["remote_binding"]["history_boundary"].is_object()
    {
        return Err(
            "this team uses an existing relay team; retry start to complete attachment".into(),
        );
    }
    let id = original["team_id"]
        .as_str()
        .filter(|id| sync::uuid(id, 7))
        .ok_or("local team has no stable UUID")?;
    if let Some(binding) = original["remote_binding"].as_object() {
        if binding.get("endpoint").and_then(Value::as_str) != Some(endpoint.as_str()) {
            return Err(
                "team is already bound to another endpoint; refusing to replace that binding"
                    .into(),
            );
        }
        if binding.get("cipher_profile").and_then(Value::as_str) != Some("none") {
            return Err("native connect supports plaintext teams only".into());
        }
    }
    let agents = original["agents"]
        .as_object()
        .ok_or("invalid local roster")?;
    let mut members = Vec::new();
    for (name, member) in agents {
        let member_id = member["member_id"]
            .as_str()
            .filter(|id| sync::uuid(id, 7))
            .ok_or("local member has no stable UUID")?;
        members.push(json!({"name":name,"member_id":member_id}));
    }
    let http = remote::client()?;
    let response = http
        .post(
            endpoint
                .join("v1/connect")
                .map_err(|_| "invalid connect endpoint")?,
        )
        .header("agmsg-protocol-version", "1")
        .json(&json!({"team_id":id,"team_name":team,"members":members,"cipher_profile":"none"}))
        .send()
        .await
        .map_err(|_| "remote registration request failed; repeat team connect to reconcile")?;
    let conflicted = response.status() == reqwest::StatusCode::CONFLICT;
    let snapshot = if conflicted {
        remote::json(
            http.get(
                endpoint
                    .join(&format!("v1/teams/{id}"))
                    .map_err(|_| "invalid team endpoint")?,
            )
            .header("agmsg-protocol-version", "1"),
        )
        .await?
    } else {
        remote::response_json(response).await?
    };
    if snapshot["protocol_version"] != 1
        || snapshot["team_id"] != id
        || snapshot["team_name"] != team
        || !snapshot["server_instance_id"]
            .as_str()
            .is_some_and(|id| sync::uuid(id, 7))
    {
        return Err("remote registration identity mismatch; local binding was preserved".into());
    }
    sync::plaintext_policy(&snapshot)?;
    if original["remote_binding"].is_object()
        && original["remote_binding"]["server_instance_id"] != snapshot["server_instance_id"]
    {
        return Err("server instance changed; local binding was preserved".into());
    }
    if conflicted {
        let remote = remote::json(
            http.get(
                endpoint
                    .join("v1/members")
                    .map_err(|_| "invalid member endpoint")?,
            )
            .header("agmsg-protocol-version", "1")
            .header("agmsg-team-id", id),
        )
        .await?;
        if remote["server_instance_id"] != snapshot["server_instance_id"]
            || remote["team_id"] != id
            || remote["protocol_version"] != 1
        {
            return Err("remote membership identity mismatch".into());
        }
        let known = remote["members"]
            .as_array()
            .ok_or("invalid remote roster")?;
        if !compatible_members(&members, known) {
            return Err(
                "remote registration has different member identities; local binding was preserved"
                    .into(),
            );
        }
    }
    let _lock = TeamLock::acquire(&root.join("teams").join(team))?;
    let mut current = registry::config(root, team)?;
    if current != original {
        return Err(
            "local team changed during registration; repeat team connect to reconcile".into(),
        );
    }
    current["remote_binding"] = json!({"endpoint":endpoint.as_str(),"server_instance_id":snapshot["server_instance_id"],"remote_team_id":id,"remote_team_name":team,
        "protocol_version":1,"cipher_profile":"none","capabilities":snapshot,"connected_at":registry::now()?,"disconnected_at":null,
        "binding_revision":original["remote_binding"]["binding_revision"].as_u64().unwrap_or(0)+1});
    registry::atomic(
        &root.join("teams").join(team).join("config.json"),
        current.to_string().as_bytes(),
    )?;
    crate::daemon::wake(root);
    println!("Connected {team}. The Rust daemon synchronizes connected teams automatically.");
    Ok(())
}

fn compatible_members(local: &[Value], remote: &[Value]) -> bool {
    // A timed-out connect can be retried after another local registration.
    // Missing remote members can be uploaded by sync after binding, but a
    // different owner of the same name must never be merged silently.
    let conflicting = local.iter().any(|l| {
        remote
            .iter()
            .any(|r| r["name"] == l["name"] && r["member_id"] != l["member_id"])
    });
    let witnessed = local.is_empty() && remote.is_empty()
        || local
            .iter()
            .any(|l| remote.iter().any(|r| r["member_id"] == l["member_id"]));
    !conflicting && witnessed
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciliation_allows_new_local_members_but_not_a_name_collision() {
        let old = json!({"name":"a","member_id":"one"});
        let new = json!({"name":"b","member_id":"two"});
        assert!(compatible_members(
            &[old.clone(), new],
            std::slice::from_ref(&old)
        ));
        assert!(!compatible_members(
            &[json!({"name":"a","member_id":"different"})],
            &[old]
        ));
    }
}
