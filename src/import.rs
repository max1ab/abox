//! Resumable download of an existing plaintext team through the v1 relay API.
use crate::{registry, remote, store::TeamLock, sync};
use serde_json::{Value, json};
use std::{fs, path::Path};

type Result<T> = std::result::Result<T, String>;
fn validate_snapshot(value: &Value, team: &str, server: &str, id: &str) -> Result<()> {
    if value["protocol_version"] != 1
        || value["server_instance_id"] != server
        || value["team_id"] != id
        || value["team_name"] != team
    {
        return Err("remote team identity changed; import refused".into());
    }
    sync::plaintext_policy(value)?;
    if value["min_available_seq"] != "0" {
        return Err(
            "remote history has a retention gap; native import cannot reconstruct it".into(),
        );
    }
    Ok(())
}

pub async fn run(root: &Path, team: &str, server: &str) -> Result<()> {
    crate::store::team_name(team)?;
    let endpoint = sync::endpoint(server)?;
    let dir = root.join("teams").join(team);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // This is the same lifecycle lock used by enable/start/stop; no command
    // can launch a competing engine while bootstrap is changing local state.
    let _control = sync::runtime::ControlLock::acquire(root, team)?;
    let existing = {
        let _lock = TeamLock::acquire(&dir)?;
        if dir
            .join("config.json")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            let config = registry::config(root, team)?;
            if config["import_pending"] != true {
                return Err("team already exists locally; use join to register an identity".into());
            }
            if config["remote_binding"]["endpoint"] != endpoint.as_str() {
                return Err("incomplete import belongs to another endpoint; retry with its original --server".into());
            }
            Some(config)
        } else {
            None
        }
    };
    let http = remote::client()?;
    let (server_id, team_id) = if let Some(ref config) = existing {
        let binding = &config["remote_binding"];
        (
            binding["server_instance_id"]
                .as_str()
                .ok_or("invalid pending server identity")?
                .to_owned(),
            binding["remote_team_id"]
                .as_str()
                .ok_or("invalid pending team identity")?
                .to_owned(),
        )
    } else {
        let mut url = endpoint
            .join("v1/teams")
            .map_err(|_| "invalid team lookup URL")?;
        url.query_pairs_mut().append_pair("name", team);
        let lookup = remote::json(http.get(url).header("agmsg-protocol-version", "1")).await?;
        if lookup["protocol_version"] != 1 || lookup["team_name"] != team {
            return Err("invalid remote team lookup".into());
        }
        let matches = lookup["teams"]
            .as_array()
            .ok_or("invalid remote team lookup")?;
        let item = match matches.as_slice() {
            [] => return Err("remote team not found".into()),
            [item] if item["team_name"] == team => item,
            _ => {
                return Err(
                    "remote team name is ambiguous; native import requires a unique name".into(),
                );
            }
        };
        (
            lookup["server_instance_id"]
                .as_str()
                .ok_or("missing server identity")?
                .to_owned(),
            item["team_id"]
                .as_str()
                .ok_or("missing team identity")?
                .to_owned(),
        )
    };
    if !sync::uuid(&server_id, 7) || !sync::uuid(&team_id, 7) {
        return Err("invalid remote server/team identity".into());
    }
    let snapshot = remote::json(
        http.get(
            endpoint
                .join(&format!("v1/teams/{team_id}"))
                .map_err(|_| "invalid team URL")?,
        )
        .header("agmsg-protocol-version", "1"),
    )
    .await?;
    validate_snapshot(&snapshot, team, &server_id, &team_id)?;
    if existing.is_none() {
        let _lock = TeamLock::acquire(&dir)?;
        // A join may have happened while HTTP was in flight. Never overwrite it.
        if dir
            .join("config.json")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            return Err("team was created concurrently; existing local state was preserved".into());
        }
        if dir
            .join("roster.jsonl")
            .try_exists()
            .map_err(|e| e.to_string())?
        {
            return Err("team has an existing roster journal; refusing to replace it".into());
        }
        let config = json!({"name":team,"team_id":team_id,"agents":{},"created_at":registry::now()?,
            "import_pending":true,"drivers":{"partition":"per-team"},
            "remote_binding":{"endpoint":endpoint.as_str(),"server_instance_id":server_id,"remote_team_id":team_id,
            "remote_team_name":team,"protocol_version":1,"cipher_profile":"none","capabilities":snapshot,
            "connected_at":registry::now()?,"disconnected_at":null,"binding_revision":1}});
        // No history may be merged accidentally into a newly imported identity.
        crate::store::initialize_import(root, team, &config)?;
    } else {
        let _lock = TeamLock::acquire(&dir)?;
        crate::store::initialize(root, team, &registry::config(root, team)?)?;
    }
    sync::bootstrap(root, team).await.map_err(|e| {
        format!("{e}; import is incomplete, retry with the same --server to resume")
    })?;
    {
        let _lock = TeamLock::acquire(&dir)?;
        let mut config = registry::config(root, team)?;
        if config["remote_binding"]["server_instance_id"] != server_id
            || config["remote_binding"]["remote_team_id"] != team_id
        {
            return Err("team binding changed during import".into());
        }
        config
            .as_object_mut()
            .ok_or("invalid team config")?
            .remove("import_pending");
        registry::atomic(&dir.join("config.json"), config.to_string().as_bytes())?;
    }
    Ok(())
}
