use reqwest::{Client, RequestBuilder, Url};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, time::Duration};

type Result<T> = std::result::Result<T, String>;

pub(crate) fn is_local(member: &Value) -> bool {
    member["registrations"].as_array().map_or_else(
        || member["type"].is_string(),
        |registrations| !registrations.is_empty(),
    )
}

pub(crate) fn member_id(member: &Value) -> Option<&str> {
    member["member_id"]
        .as_str()
        .filter(|id| crate::sync::uuid(id, 7))
}

pub(crate) struct Relay<'a> {
    base: Url,
    server: &'a str,
    team: &'a str,
}

impl<'a> Relay<'a> {
    pub(crate) fn from_binding(binding: &'a Value) -> Result<Self> {
        if binding["protocol_version"] != 1 || binding["cipher_profile"] != "none" {
            return Err("presence requires a protocol 1 plaintext binding".into());
        }
        let endpoint = binding["endpoint"]
            .as_str()
            .ok_or("missing relay endpoint")?;
        let identity = |field: &str| {
            binding[field]
                .as_str()
                .filter(|id| crate::sync::uuid(id, 7))
                .ok_or("invalid connected server/team identity")
        };
        Ok(Self {
            base: crate::sync::endpoint(endpoint)?,
            server: identity("server_instance_id")?,
            team: identity("remote_team_id")?,
        })
    }

    pub(crate) fn request(&self, client: &Client, path: &str) -> Result<RequestBuilder> {
        let url = self.base.join(path).map_err(|_| "invalid relay route")?;
        Ok(client.get(url).timeout(Duration::from_secs(3)))
    }

    pub(crate) async fn json(&self, request: RequestBuilder) -> Result<Value> {
        crate::remote::team_json(request, self.server, self.team).await
    }

    pub(crate) async fn query(
        &self,
        client: &Client,
        member: Option<&str>,
    ) -> Result<BTreeSet<String>> {
        let mut request = self.request(client, "v1/agent-mail/presence")?;
        if let Some(id) = member {
            request = request.query(&[("member_id", id)]);
        }
        let value = self.json(request).await?;
        value["members"]
            .as_array()
            .ok_or("invalid presence response")?
            .iter()
            .map(|id| {
                id.as_str()
                    .filter(|id| crate::sync::uuid(id, 7))
                    .map(str::to_owned)
                    .ok_or_else(|| "invalid presence member ID".into())
            })
            .collect()
    }
}

pub(crate) async fn list(root: &Path, team: Option<String>, all: bool) -> Result<()> {
    let teams = match team {
        Some(team) => vec![team],
        None => crate::mailbox::teams(root)?
            .iter()
            .map(|team| {
                team["name"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("invalid team name")
            })
            .collect::<std::result::Result<_, _>>()?,
    };
    let mut rosters = Vec::new();
    let mut probes = tokio::task::JoinSet::new();
    for (index, name) in teams.into_iter().enumerate() {
        let config = crate::registry::config(root, &name)?;
        let members = crate::mailbox::members_from_config(&config)?;
        let binding = &config["remote_binding"];
        if !all
            && config["attachment_pending"] != true
            && binding.is_object()
            && binding.get("disconnected_at") == Some(&Value::Null)
        {
            let binding = binding.clone();
            probes.spawn(async move {
                let result = async {
                    Relay::from_binding(&binding)?
                        .query(&crate::remote::client()?, None)
                        .await
                }
                .await;
                (index, result)
            });
        }
        rosters.push((name, config, members, BTreeSet::new()));
    }
    while let Some(result) = probes.join_next().await {
        let (index, result) = result.map_err(|_| "presence check interrupted")?;
        let (team, _, _, reachable) = &mut rosters[index];
        match result {
            Ok(members) => *reachable = members,
            Err(error) => {
                eprintln!("agent-mail: could not check remote members in team '{team}': {error}")
            }
        }
    }
    crate::client::print_jsonl(rosters.into_iter().flat_map(|(team, config, members, reachable)| {
        members.into_iter().filter_map(move |member| {
            let registration = &config["agents"][&member.name];
            (all || is_local(registration) || member_id(registration).is_some_and(|id| reachable.contains(id)))
                .then(|| json!({"team":team,"name":member.name,"types":member.types,"project":member.project}))
        })
    }))
}
