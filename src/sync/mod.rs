//! Native transport for the pinned agmsg SQLite and HTTP contracts.
//! Reservations and cursors live in upstream-compatible tables, so taking over
//! an already-synced mailbox does not invent new wire IDs or replay history.
use crate::store::{TeamLock, TeamStore};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, Method, Url};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Envelope {
    v: u32,
    cipher: String,
    key_id: Option<String>,
    blob: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Candidate {
    id: String,
    envelope: Envelope,
}
#[derive(Deserialize)]
struct RemoteMessage {
    id: String,
    server_seq: String,
    envelope: Envelope,
    #[serde(default)]
    server_received_at: String,
}
#[derive(Deserialize, Serialize)]
struct Projection {
    body: String,
    created_at: String,
    from_agent: String,
    to_agent: String,
}
#[derive(Serialize)]
pub struct Outcome {
    uploaded: usize,
    roster_uploaded: usize,
    downloaded: usize,
    transport_cursor: String,
    has_more: bool,
}

struct Context {
    store: TeamStore,
    team: String,
    server: String,
    remote: String,
    generation: String,
    base_url: Url,
    http: Client,
}
fn sql<T>(result: rusqlite::Result<T>) -> Result<T> {
    result.map_err(|e| format!("sync storage: {e}"))
}
fn sequence(value: &str) -> Result<i64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|c| c.is_ascii_digit())
    {
        return Err("invalid remote sequence".into());
    }
    value
        .parse::<i64>()
        .map_err(|_| "remote sequence exceeds supported range".into())
}
fn uuid(value: &str, version: usize) -> bool {
    uuid::Uuid::parse_str(value)
        .is_ok_and(|u| u.get_version_num() == version && u.to_string() == value)
}
fn binding_string(config: &Value, field: &str) -> Result<String> {
    config
        .pointer(&format!("/remote_binding/{field}"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing connected binding: {field}"))
}
fn endpoint(raw: &str) -> Result<Url> {
    let mut url = Url::parse(raw).map_err(|_| "invalid remote endpoint")?;
    if url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("remote endpoint must not contain credentials, query or fragment".into());
    }
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("remote endpoint must use HTTP(S)".into());
    }
    if url.scheme() == "http" {
        let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
        let private = host == "localhost"
            || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_private() || ip.is_loopback() || ip.is_link_local()
                }
                std::net::IpAddr::V6(ip) => {
                    ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
                }
            });
        if !private {
            return Err(
                "plaintext HTTP requires a private IP or localhost; use HTTPS or an SSH tunnel"
                    .into(),
            );
        }
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

impl Context {
    fn open(root: &Path, team: &str) -> Result<Self> {
        Self::open_for_adoption(root, team, false)
    }
    fn open_for_adoption(root: &Path, team: &str, allow_official: bool) -> Result<Self> {
        if let Some(connection_root) =
            std::env::var_os("AGMSG_SYNC_CONNECTION_DIR").filter(|p| !p.is_empty())
        {
            let selected = crate::store::native_path(Path::new(&connection_root))
                .canonicalize()
                .map_err(|e| format!("cannot validate AGMSG_SYNC_CONNECTION_DIR: {e}"))?;
            let expected = root.canonicalize().map_err(|e| e.to_string())?;
            if selected != expected {
                return Err("AGMSG_SYNC_CONNECTION_DIR points to another installation; use its --agmsg-dir or clear the override before native sync".into());
            }
        }
        let mut store = crate::store::open(root, team)?;
        // Do not run two engines over one binding. The official stop command
        // removes this pidfile; a leftover file requires explicit recovery.
        if root
            .join("run")
            .join(format!("remote-sync.{team}.pid"))
            .exists()
            && !allow_official
        {
            return Err("the official remote sync engine is selected; use agent-mail sync enable TEAM to stop it and adopt native sync".into());
        }
        let config = &store.config;
        if config.pointer("/remote_binding/disconnected_at") != Some(&Value::Null)
            || config
                .pointer("/remote_binding/protocol_version")
                .and_then(Value::as_u64)
                != Some(1)
            || config
                .pointer("/remote_binding/cipher_profile")
                .and_then(Value::as_str)
                != Some("none")
        {
            return Err("native sync currently requires a connected protocol 1 plaintext binding; encrypted teams are left untouched".into());
        }
        let server = binding_string(config, "server_instance_id")?;
        let remote = binding_string(config, "remote_team_id")?;
        if !uuid(&server, 7) || !uuid(&remote, 7) {
            return Err("invalid connected server/team identity".into());
        }
        let base_url = endpoint(&binding_string(config, "endpoint")?)?;
        let encoded: String = team
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let authority = store
            .base
            .join("remote-sync")
            .join(format!("{encoded}.json"));
        if authority.exists() {
            let saved: Value =
                serde_json::from_slice(&fs::read(authority).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if saved["server_instance_id"] != server
                || saved["remote_team_id"] != remote
                || saved["server_url"] != binding_string(config, "endpoint")?
                || saved["cipher_profile"] != "none"
                || saved["local_team"] != team
                || saved["protocol_version"] != 1
                || saved["local_security_history"].as_array().is_none_or(|h| {
                    h.is_empty()
                        || h.iter()
                            .any(|r| r["minimum_security_mode"] != "plaintext-allowed")
                })
            {
                return Err(
                    "stored official sync authority does not match the plaintext binding".into(),
                );
            }
        }
        sql(store.db.execute_batch(include_str!("schema.sql")))?;
        sql(store.db.execute(
            "INSERT OR IGNORE INTO sync_store_metadata(singleton,generation) VALUES (1,?1)",
            [uuid::Uuid::new_v4().to_string()],
        ))?;
        let generation: String = sql(store.db.query_row(
            "SELECT generation FROM sync_store_metadata WHERE singleton=1",
            [],
            |r| r.get(0),
        ))?;
        sql(store.db.execute("INSERT OR IGNORE INTO sync_bindings(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation) VALUES (?1,?2,?3,1,?4)", params![team,server,remote,generation]))?;
        // Never hold the team registry lock across HTTP requests.
        drop(store.registry_lock.take());
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            store,
            team: team.into(),
            server,
            remote,
            generation,
            base_url,
            http,
        })
    }
    fn guard(&self) -> Result<TeamLock> {
        let lock = TeamLock::acquire(&self.store.team_dir)?;
        let current: Value = serde_json::from_slice(
            &fs::read(self.store.team_dir.join("config.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if current["remote_binding"] != self.store.config["remote_binding"]
            || current["drivers"] != self.store.config["drivers"]
        {
            return Err("team binding changed during sync; retry with the current binding".into());
        }
        Ok(lock)
    }
    async fn request(&self, method: Method, route: &str, body: Option<Value>) -> Result<Value> {
        let url = self
            .base_url
            .join(route)
            .map_err(|_| "invalid protocol route")?;
        let mut request = self
            .http
            .request(method, url)
            .header("agmsg-protocol-version", "1")
            .header("agmsg-team-id", &self.remote);
        if let Some(body) = body {
            request = request.json(&body);
        }
        // Do not print URLs or reqwest errors: they may carry private endpoint details.
        let mut response = request.send().await.map_err(|_| "remote request failed")?;
        if !response.status().is_success() {
            return Err(format!(
                "remote returned HTTP {}; no cursor was advanced",
                response.status()
            ));
        }
        if response
            .headers()
            .get("agmsg-protocol-version")
            .and_then(|s| s.to_str().ok())
            != Some("1")
        {
            return Err("remote omitted protocol version".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "cannot read remote response")?
        {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err("remote response exceeds 16 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid remote JSON")?;
        if value["server_instance_id"] != self.server
            || value["team_id"] != self.remote
            || value["protocol_version"] != 1
        {
            return Err("remote binding changed; refusing to write or advance cursors".into());
        }
        Ok(value)
    }
    fn cursor(&self) -> Result<String> {
        sql(self.store.db.query_row("SELECT transport_cursor FROM sync_bindings WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4",
            params![self.team,self.server,self.remote,self.generation], |r|r.get(0)))
    }
}

pub mod bridge;
mod events;
mod read_state;
mod roster;
pub mod runtime;
pub mod service;

pub async fn once(root: &Path, team: &str) -> Result<Outcome> {
    let _lock = runtime::EngineLock::acquire(root, team)?;
    cycle(root, team).await
}

async fn cycle(root: &Path, team: &str) -> Result<Outcome> {
    let mut ctx = Context::open(root, team)?;
    let caps = ctx.request(Method::GET, "v1/capabilities", None).await?;
    if !caps["accepted_envelope_versions"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == 1))
        || caps["cipher_profile"] != "none"
        || !caps["write_allowed_ciphers"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == "none"))
    {
        return Err("remote encryption policy does not allow this plaintext binding".into());
    }
    let floor = caps["min_available_seq"]
        .as_str()
        .ok_or("missing retention floor")?;
    let minimum = sequence(&ctx.cursor()?)?.min(sequence(&ctx.roster_cursor()?)?);
    if minimum < sequence(floor)? {
        return Err(
            "remote history was retained past the local cursor; explicit gap recovery is required"
                .into(),
        );
    }
    let max_blob = caps["max_blob_bytes"]
        .as_str()
        .and_then(|x| x.parse::<usize>().ok())
        .filter(|n| *n > 0 && *n <= 1_048_576)
        .ok_or("invalid remote blob limit")?;
    let roster_pending = ctx.reserve_roster(max_blob)?;
    let roster_uploaded = roster_pending.len();
    for candidate in roster_pending {
        let response = ctx
            .request(
                Method::POST,
                "v1/messages",
                Some(json!({"messages":[candidate]})),
            )
            .await?;
        ctx.ack_roster(&candidate, &response)?;
    }
    // Reserve before POST; every retry reuses the exact stored ID and envelope.
    let pending = ctx.reserve(max_blob)?;
    let mut uploaded = 0;
    for candidate in pending {
        let response = ctx
            .request(
                Method::POST,
                "v1/messages",
                Some(json!({"messages":[candidate]})),
            )
            .await?;
        ctx.ack(&candidate, &response)?;
        uploaded += 1;
    }
    let expected = ctx.cursor()?;
    let after = sequence(&expected)?
        .min(sequence(&ctx.roster_cursor()?)?)
        .to_string();
    let page = ctx
        .request(
            Method::GET,
            &format!("v1/messages?after={after}&limit=10"),
            None,
        )
        .await?;
    let downloaded = ctx.apply(&expected, &after, &page)?;
    ctx.read_state().await?;
    Ok(Outcome {
        uploaded,
        roster_uploaded,
        downloaded,
        transport_cursor: ctx.cursor()?,
        has_more: page["has_more"] == true || uploaded == 10 || roster_uploaded == 10,
    })
}

fn canonical_time(input: &str) -> Result<String> {
    if !input.is_ascii() || !input.ends_with('Z') || input.len() < 20 {
        return Err("invalid message timestamp".into());
    }
    let base = &input[..19];
    if base.as_bytes()[4] != b'-'
        || base.as_bytes()[7] != b'-'
        || base.as_bytes()[10] != b'T'
        || base.as_bytes()[13] != b':'
        || base.as_bytes()[16] != b':'
    {
        return Err("invalid message timestamp".into());
    }
    let num = |a: usize, b: usize| {
        base[a..b]
            .parse::<u32>()
            .map_err(|_| "invalid timestamp component".to_owned())
    };
    let (y, m, d, h, min, sec) = (
        num(0, 4)?,
        num(5, 7)?,
        num(8, 10)?,
        num(11, 13)?,
        num(14, 16)?,
        num(17, 19)?,
    );
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) => 29,
        2 => 28,
        _ => 0,
    };
    if d == 0 || d > days || h > 23 || min > 59 || sec > 59 {
        return Err("invalid message timestamp".into());
    }
    let frac = if input.len() == 20 {
        ""
    } else {
        if input.as_bytes()[19] != b'.' {
            return Err("invalid message timestamp".into());
        }
        &input[20..input.len() - 1]
    };
    if frac.len() > 6 || !frac.bytes().all(|c| c.is_ascii_digit()) {
        return Err("invalid timestamp fraction".into());
    }
    Ok(format!("{base}.{frac:0<6}Z"))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 128
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|c| c.is_control() || "./\\\"[]".contains(c))
}
fn plain(envelope: &Envelope) -> Result<Value> {
    if envelope.v != 1 || envelope.cipher != "none" || envelope.key_id.is_some() {
        return Err("unsupported encrypted or future envelope; cursor preserved".into());
    }
    let bytes = STANDARD
        .decode(&envelope.blob)
        .map_err(|_| "invalid envelope base64")?;
    if bytes.is_empty() || bytes.len() > 1_048_576 || STANDARD.encode(&bytes) != envelope.blob {
        return Err("noncanonical envelope base64".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid message projection".into())
}
fn projection(envelope: &Envelope) -> Result<Projection> {
    let raw = plain(envelope)?;
    let p: Projection =
        serde_json::from_value(raw).map_err(|_| "invalid message projection fields")?;
    if p.body.is_empty()
        || p.body.len() > 1_000_000
        || p.body.contains('\0')
        || !valid_name(&p.from_agent)
        || !valid_name(&p.to_agent)
        || canonical_time(&p.created_at)? != p.created_at
    {
        return Err("invalid message projection content".into());
    }
    Ok(p)
}
impl Context {
    fn reserve(&mut self, max_blob: usize) -> Result<Vec<Candidate>> {
        let _guard = self.guard()?;
        let tx = sql(self
            .store
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        crate::store::validate(&tx)?;
        // Bootstrap legacy-only history exactly once, preserving the official
        // negative event positions and existing wire reservations.
        let too_large: bool = sql(tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE team=?1 AND id>=1000000000)",
            [&self.team],
            |r| r.get(0),
        ))?;
        if too_large {
            return Err("legacy ID exceeds the supported projection offset".into());
        }
        sql(tx.execute("INSERT INTO events(seq,type,id,team,from_agent,to_agent,body,at,legacy_id)
            SELECT m.id-1000000000,'message_sent',CAST(m.id AS TEXT),m.team,m.from_agent,m.to_agent,m.body,m.created_at,m.id FROM messages m
            WHERE m.team=?1 AND NOT EXISTS(SELECT 1 FROM events e WHERE e.legacy_id=m.id)
            AND NOT EXISTS(SELECT 1 FROM events e WHERE e.seq=m.id-1000000000)",[&self.team]))?;
        let rows = {
            let mut statement=sql(tx.prepare("SELECT e.seq,e.id,e.body,e.at,e.from_agent,e.to_agent FROM events e
              LEFT JOIN sync_messages m ON m.local_team=?1 AND m.server_instance_id=?2 AND m.remote_team_id=?3
              AND m.protocol_version=1 AND m.driver_generation=?4 AND m.local_position=e.seq
              WHERE e.team=?1 AND e.type='message_sent' AND m.server_seq IS NULL ORDER BY e.seq LIMIT 10"))?;
            sql(statement.query_map(
                params![self.team, self.server, self.remote, self.generation],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                    ))
                },
            ))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?
        };
        let mut candidates = Vec::new();
        for (position, local_id, body, at, from, to) in rows {
            let reserved=sql(tx.query_row("SELECT wire_id,envelope_v,cipher,key_id,blob FROM sync_messages WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4 AND local_position=?5",
              params![self.team,self.server,self.remote,self.generation,position],|r|Ok(Candidate {id:r.get(0)?,envelope:Envelope {v:r.get(1)?,cipher:r.get(2)?,key_id:r.get(3)?,blob:r.get(4)?}})).optional())?;
            let candidate = if let Some(candidate) = reserved {
                candidate
            } else {
                let p = Projection {
                    body,
                    created_at: canonical_time(&at)?,
                    from_agent: from,
                    to_agent: to,
                };
                let candidate = Candidate {
                    id: uuid::Uuid::new_v4().to_string(),
                    envelope: Envelope {
                        v: 1,
                        cipher: "none".into(),
                        key_id: None,
                        blob: STANDARD.encode(serde_json::to_vec(&p).map_err(|e| e.to_string())?),
                    },
                };
                projection(&candidate.envelope)?;
                sql(tx.execute("INSERT INTO sync_messages(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,local_position,local_id,wire_id,envelope_v,cipher,key_id,blob,server_seq,direction)
                  VALUES (?1,?2,?3,1,?4,?5,?6,?7,1,'none',NULL,?8,NULL,'push')",params![self.team,self.server,self.remote,self.generation,position,local_id,candidate.id,candidate.envelope.blob]))?;
                candidate
            };
            if !uuid(&candidate.id, 4) {
                return Err("stored wire ID is not UUIDv4".into());
            }
            if STANDARD
                .decode(&candidate.envelope.blob)
                .map_err(|_| "invalid reserved blob")?
                .len()
                > max_blob
            {
                return Err("reserved message exceeds current remote blob limit".into());
            }
            // Unsupported prior reservations must never be re-encrypted or
            // replaced under a new UUID merely to get an upload through.
            projection(&candidate.envelope)?;
            candidates.push(candidate);
        }
        sql(tx.commit())?;
        Ok(candidates)
    }
    fn ack(&mut self, candidate: &Candidate, response: &Value) -> Result<()> {
        let seq = ack_sequence(candidate, response)?;
        let _guard = self.guard()?;
        let tx = sql(self
            .store
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let updated=sql(tx.execute("UPDATE sync_messages SET server_seq=?1 WHERE local_team=?2 AND server_instance_id=?3 AND remote_team_id=?4 AND protocol_version=1 AND driver_generation=?5 AND wire_id=?6 AND envelope_v=?7 AND cipher=?8 AND key_id IS ?9 AND blob=?10 AND (server_seq IS NULL OR server_seq=?1)",
            params![seq,self.team,self.server,self.remote,self.generation,candidate.id,candidate.envelope.v,candidate.envelope.cipher,candidate.envelope.key_id,candidate.envelope.blob]))?;
        if updated != 1 {
            return Err("upload reservation changed; acknowledgment not applied".into());
        }
        sql(tx.commit())
    }
    fn apply(&mut self, expected: &str, after: &str, page: &Value) -> Result<usize> {
        let messages: Vec<RemoteMessage> = serde_json::from_value(page["messages"].clone())
            .map_err(|_| "invalid remote message page")?;
        if messages.len() > 10 {
            return Err("remote exceeded requested page size".into());
        }
        let next = page["next_after"].as_str().ok_or("missing next cursor")?;
        let mut last = sequence(after)?;
        for message in &messages {
            let seq = sequence(&message.server_seq)?;
            if seq <= last || !uuid(&message.id, 4) {
                return Err("remote message IDs or sequence ordering invalid".into());
            }
            let value = plain(&message.envelope)?;
            if value.get("kind").is_some() {
                roster::validate(&value)?;
            } else {
                projection(&message.envelope)?;
            }
            last = seq;
        }
        if sequence(next)? != last
            || page["has_more"].as_bool().is_none()
            || (messages.is_empty() && page["has_more"] == true)
        {
            return Err("inconsistent remote page cursor".into());
        }
        let _guard = self.guard()?;
        let mut roster = roster::Roster::load(self)?;
        let tx = sql(self
            .store
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        crate::store::validate(&tx)?;
        let cursor:String=sql(tx.query_row("SELECT transport_cursor FROM sync_bindings WHERE local_team=?1 AND server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND driver_generation=?4",params![self.team,self.server,self.remote,self.generation],|r|r.get(0)))?;
        if cursor != expected {
            return Err("transport cursor changed concurrently; retry".into());
        }
        let mut imported = 0;
        for message in messages {
            let corrupt: bool = sql(tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_conflicts WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND (server_seq=?3 OR wire_id=?4))
                 OR EXISTS(SELECT 1 FROM sync_quarantine WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND (server_seq=?3 OR wire_id=?4) AND status='corrupt_state')
                 OR EXISTS(SELECT 1 FROM sync_messages WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND wire_id=?4 AND (local_team<>?5 OR driver_generation<>?6))",
                params![self.server,self.remote,message.server_seq,message.id,self.team,self.generation], |r|r.get(0)))?;
            if corrupt {
                return Err("conflicting sync state; explicit recovery required".into());
            }
            let value = plain(&message.envelope)?;
            if value.get("kind").is_some() {
                let prior=sql(tx.query_row("SELECT envelope_v,cipher,key_id,blob,server_seq FROM sync_quarantine WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND wire_id=?3",params![self.server,self.remote,message.id],|r|Ok((Envelope{v:r.get(0)?,cipher:r.get(1)?,key_id:r.get(2)?,blob:r.get(3)?},r.get::<_,String>(4)?))).optional())?;
                if let Some((envelope, seq)) = prior
                    && (envelope != message.envelope || seq != message.server_seq)
                {
                    return Err("roster envelope conflicts with stored state".into());
                }
                let conflicting:bool=sql(tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_messages WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND (wire_id=?3 OR server_seq=?4)) OR EXISTS(SELECT 1 FROM sync_quarantine WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND server_seq=?4 AND wire_id<>?3)",params![self.server,self.remote,message.id,message.server_seq],|r|r.get(0)))?;
                if conflicting {
                    return Err("roster sequence conflicts with existing sync state".into());
                }
                roster.apply(&value, &message.server_seq, &message.id)?;
                sql(tx.execute("INSERT OR IGNORE INTO sync_quarantine(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,server_seq,wire_id,server_received_at,envelope_v,cipher,key_id,blob,status) VALUES (?1,?2,?3,1,?4,?5,?6,?7,?8,?9,?10,?11,'imported')",params![self.team,self.server,self.remote,self.generation,message.server_seq,message.id,message.server_received_at,message.envelope.v,message.envelope.cipher,message.envelope.key_id,message.envelope.blob]))?;
                continue;
            }
            let existing=sql(tx.query_row("SELECT envelope_v,cipher,key_id,blob,server_seq FROM sync_messages WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND wire_id=?3",
                params![self.server,self.remote,message.id],|r|Ok((Envelope {v:r.get(0)?,cipher:r.get(1)?,key_id:r.get(2)?,blob:r.get(3)?},r.get::<_,Option<String>>(4)?))).optional())?;
            if let Some((envelope, seq)) = existing {
                if envelope != message.envelope
                    || seq.as_deref().is_some_and(|s| s != message.server_seq)
                {
                    return Err("remote envelope conflicts with an existing mapping".into());
                }
                sql(tx.execute("UPDATE sync_messages SET server_seq=?1 WHERE server_instance_id=?2 AND remote_team_id=?3 AND protocol_version=1 AND wire_id=?4",params![message.server_seq,self.server,self.remote,message.id]))?;
                continue;
            }
            let conflict:bool=sql(tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_messages WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND server_seq=?3)
              OR EXISTS(SELECT 1 FROM sync_quarantine WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND (server_seq=?3 OR wire_id=?4))
              OR EXISTS(SELECT 1 FROM sync_conflicts WHERE server_instance_id=?1 AND remote_team_id=?2 AND protocol_version=1 AND (server_seq=?3 OR wire_id=?4))",
              params![self.server,self.remote,message.server_seq,message.id],|r|r.get(0)))?;
            if conflict {
                return Err("remote page conflicts with prior sync/quarantine state; explicit recovery required".into());
            }
            let p = projection(&message.envelope)?;
            let local_id = uuid::Uuid::now_v7().to_string();
            sql(tx.execute("INSERT INTO messages(team,from_agent,to_agent,body,created_at) VALUES (?1,?2,?3,?4,?5)",params![self.team,p.from_agent,p.to_agent,p.body,p.created_at]))?;
            let legacy_id = tx.last_insert_rowid();
            sql(tx.execute("INSERT INTO events(type,id,team,from_agent,to_agent,body,at,legacy_id) VALUES ('message_sent',?1,?2,?3,?4,?5,?6,?7)",params![local_id,self.team,p.from_agent,p.to_agent,p.body,p.created_at,legacy_id]))?;
            let position = tx.last_insert_rowid();
            sql(tx.execute("INSERT INTO sync_messages(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,local_position,local_id,wire_id,envelope_v,cipher,key_id,blob,server_seq,direction)
              VALUES (?1,?2,?3,1,?4,?5,?6,?7,?8,?9,?10,?11,?12,'pull')",params![self.team,self.server,self.remote,self.generation,position,local_id,message.id,message.envelope.v,message.envelope.cipher,message.envelope.key_id,message.envelope.blob,message.server_seq]))?;
            sql(tx.execute("INSERT OR IGNORE INTO sync_read_aliases(local_team,server_instance_id,remote_team_id,protocol_version,driver_generation,agent,local_id,wire_id,server_seq)
              VALUES (?1,?2,?3,1,?4,?5,?6,?7,?8)",params![self.team,self.server,self.remote,self.generation,p.to_agent,local_id,message.id,message.server_seq]))?;
            imported += 1;
        }
        roster.advance(next)?;
        roster.save()?;
        let next = sequence(expected)?.max(sequence(next)?).to_string();
        sql(tx.execute("UPDATE sync_bindings SET transport_cursor=?1 WHERE local_team=?2 AND server_instance_id=?3 AND remote_team_id=?4 AND protocol_version=1 AND driver_generation=?5",params![next,self.team,self.server,self.remote,self.generation]))?;
        sql(tx.commit())?;
        Ok(imported)
    }
}

#[cfg(test)]
mod tests;

fn ack_sequence(candidate: &Candidate, response: &Value) -> Result<String> {
    let acks = response["acks"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or("invalid upload acknowledgment")?;
    let ack = &acks[0];
    if ack["id"] != candidate.id
        || !matches!(ack["disposition"].as_str(), Some("stored" | "duplicate"))
    {
        return Err("upload acknowledgment does not match reservation".into());
    }
    let seq = ack["server_seq"]
        .as_str()
        .ok_or("ack omitted server sequence")?;
    if sequence(seq)? == 0 {
        return Err("ack sequence must be positive".into());
    }
    Ok(seq.to_owned())
}
