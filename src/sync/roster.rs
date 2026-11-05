//! Compatible with agmsg v1.7.0 roster.jsonl / roster-sync.json. The journal is
//! authoritative; config is a derived cache preserving local registrations.
use super::{Candidate, Context, Result, canonical_time, sequence, uuid, valid_name};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(super) fn validate(value: &Value) -> Result<()> {
    let kind = value["kind"].as_str().ok_or("roster kind is missing")?;
    let names: &[&str] = match kind {
        "member_joined" | "member_left" => {
            &["kind", "mutation_id", "member_id", "name", "occurred_at"]
        }
        "member_renamed" => &[
            "kind",
            "mutation_id",
            "member_id",
            "from",
            "to",
            "occurred_at",
        ],
        _ => return Err("unsupported roster mutation or key rotation; cursor preserved".into()),
    };
    let object = value.as_object().ok_or("invalid roster mutation")?;
    if object.len() != names.len() || names.iter().any(|k| !object.contains_key(*k)) {
        return Err("unexpected roster fields".into());
    }
    if !value["mutation_id"].as_str().is_some_and(|v| uuid(v, 7))
        || !value["member_id"].as_str().is_some_and(|v| uuid(v, 7))
    {
        return Err("invalid roster identity".into());
    }
    let at = value["occurred_at"]
        .as_str()
        .ok_or("invalid roster timestamp")?;
    if canonical_time(at)? != at {
        return Err("roster timestamp must be canonical".into());
    }
    for key in if kind == "member_renamed" {
        &["from", "to"][..]
    } else {
        &["name"][..]
    } {
        if !value[*key].as_str().is_some_and(valid_name) {
            return Err("invalid roster name".into());
        }
    }
    if kind == "member_renamed" && value["from"] == value["to"] {
        return Err("roster rename is a no-op".into());
    }
    Ok(())
}
fn projection(record: &Value) -> Result<Value> {
    let mut value = json!({"kind":record["type"],"mutation_id":record["id"],"member_id":record["member_id"],
        "occurred_at":canonical_time(record["at"].as_str().ok_or("invalid roster journal time")?)?});
    if record["type"] == "member_renamed" {
        value["from"] = record["from"].clone();
        value["to"] = record["to"].clone();
    } else {
        value["name"] = record["name"].clone();
    }
    validate(&value)?;
    Ok(value)
}
fn mutation(value: &Value) -> Value {
    let mut record = json!({"type":value["kind"],"id":value["mutation_id"],"member_id":value["member_id"],"at":value["occurred_at"]});
    if value["kind"] == "member_renamed" {
        record["from"] = value["from"].clone();
        record["to"] = value["to"].clone();
    } else {
        record["name"] = value["name"].clone();
    }
    record
}
fn is_mutation(record: &Value) -> bool {
    matches!(
        record["type"].as_str(),
        Some("member_joined" | "member_left" | "member_renamed" | "key_rotated")
    )
}
pub(crate) fn validate_journal(records: &[Value]) -> Result<()> {
    for record in records {
        match record["type"].as_str() {
            Some("member_joined" | "member_left" | "member_renamed" | "key_rotated") => {
                projection(record)?;
            }
            Some("roster_synced") => {}
            _ => return Err("unsupported or malformed roster journal record".into()),
        }
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if read_optional(path)?.as_deref() == Some(bytes) {
        return Ok(());
    }
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        fs::File::open(path.parent().ok_or("missing roster directory")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub(super) struct Roster {
    dir: PathBuf,
    records: Vec<Value>,
    state: Value,
    key: String,
    server: String,
    remote: String,
}
impl Roster {
    // Caller holds the official team registry lock for load/mutate/save.
    pub(super) fn load(ctx: &Context) -> Result<Self> {
        let dir = ctx.store.team_dir.clone();
        let records = match read_optional(&dir.join("roster.jsonl"))? {
            Some(bytes) => {
                let text = String::from_utf8(bytes).map_err(|_| "invalid roster journal UTF-8")?;
                text.lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| {
                        serde_json::from_str(line)
                            .map_err(|_| "invalid roster journal JSON".to_string())
                    })
                    .collect::<Result<Vec<Value>>>()?
            }
            None => {
                let cfg: Value = serde_json::from_slice(
                    &fs::read(dir.join("config.json")).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                let agents = cfg["agents"].as_object().ok_or("invalid team roster")?;
                let at: String = ctx
                    .store
                    .db
                    .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
                        r.get(0)
                    })
                    .map_err(|e| e.to_string())?;
                agents.iter().map(|(name,agent)|json!({"type":"member_joined","id":uuid::Uuid::now_v7().to_string(),"member_id":agent["member_id"],"name":name,"at":at})).collect()
            }
        };
        for record in records.iter().filter(|r| is_mutation(r)) {
            projection(record)?;
        }
        let mut state = match read_optional(&dir.join("roster-sync.json"))? {
            Some(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(
                |_| "invalid roster sync state; recover with the official tool before native sync",
            )?,
            None => json!({"format_version":1,"bindings":{}}),
        };
        if state["format_version"] != 1 || !state["bindings"].is_object() {
            return Err("unsupported roster sync state".into());
        }
        let key = format!("{}:{}:1", ctx.server, ctx.remote);
        if state["bindings"].get(&key).is_none() {
            state["bindings"][&key] = json!({"reservations":{},"transport_cursor":"0"});
        }
        if !state["bindings"][&key]["reservations"].is_object() {
            return Err("invalid roster reservations".into());
        }
        let result = Self {
            dir,
            records,
            state,
            key,
            server: ctx.server.clone(),
            remote: ctx.remote.clone(),
        };
        result.cursor()?;
        Ok(result)
    }
    pub(super) fn cursor(&self) -> Result<String> {
        let cursor = self.state["bindings"][&self.key]["transport_cursor"]
            .as_str()
            .ok_or("invalid roster transport cursor")?;
        sequence(cursor)?;
        Ok(cursor.into())
    }
    fn synced(&self, id: &Value) -> bool {
        self.records.iter().any(|r| {
            r["type"] == "roster_synced"
                && r["mutation_id"] == *id
                && r["server_instance_id"] == self.server
                && r["remote_team_id"] == self.remote
        })
    }
    fn sync_record(&mut self, id: &Value, seq: &str, wire: &str) {
        if !self.synced(id) {
            self.records.push(json!({"type":"roster_synced","mutation_id":id,"server_seq":seq,"wire_id":wire,"server_instance_id":self.server,"remote_team_id":self.remote}));
        }
    }
    pub(super) fn prepare(&mut self, max_blob: usize) -> Result<Vec<Candidate>> {
        let pending: Vec<Value> = self
            .records
            .iter()
            .filter(|r| is_mutation(r) && !self.synced(&r["id"]))
            .take(10)
            .cloned()
            .collect();
        let mut result = Vec::new();
        for record in pending {
            let id = record["id"].as_str().ok_or("invalid roster mutation ID")?;
            let expected = projection(&record)?;
            if self.state["bindings"][&self.key]["reservations"]
                .get(id)
                .is_none()
            {
                let blob =
                    STANDARD.encode(serde_json::to_vec(&expected).map_err(|e| e.to_string())?);
                self.state["bindings"][&self.key]["reservations"][id] = json!({"wire_id":uuid::Uuid::new_v4().to_string(),"envelope":{"v":1,"cipher":"none","key_id":null,"blob":blob}});
            }
            let reserved = &self.state["bindings"][&self.key]["reservations"][id];
            let candidate = Candidate {
                id: reserved["wire_id"]
                    .as_str()
                    .ok_or("invalid reserved roster wire ID")?
                    .into(),
                envelope: serde_json::from_value(reserved["envelope"].clone())
                    .map_err(|_| "invalid reserved roster envelope")?,
            };
            if !uuid(&candidate.id, 4)
                || super::plain(&candidate.envelope)? != expected
                || STANDARD
                    .decode(&candidate.envelope.blob)
                    .map_err(|_| "invalid roster base64")?
                    .len()
                    > max_blob
            {
                return Err("roster reservation no longer matches the journal/policy".into());
            }
            result.push(candidate);
        }
        Ok(result)
    }
    pub(super) fn ack(&mut self, candidate: &Candidate, seq: &str) -> Result<()> {
        let projection = super::plain(&candidate.envelope)?;
        validate(&projection)?;
        let id = projection["mutation_id"]
            .as_str()
            .ok_or("invalid roster mutation ID")?;
        let reserved = &mut self.state["bindings"][&self.key]["reservations"][id];
        if reserved["wire_id"] != candidate.id
            || reserved["envelope"]
                != serde_json::to_value(&candidate.envelope).map_err(|e| e.to_string())?
        {
            return Err("roster ACK does not match durable reservation".into());
        }
        reserved["server_seq"] = json!(seq);
        self.sync_record(&projection["mutation_id"], seq, &candidate.id);
        Ok(())
    }
    pub(super) fn apply(&mut self, value: &Value, seq: &str, wire: &str) -> Result<()> {
        validate(value)?;
        if let Some(prior) = self
            .records
            .iter()
            .find(|r| is_mutation(r) && r["id"] == value["mutation_id"])
        {
            if projection(prior)? != *value {
                return Err("roster mutation ID conflicts with prior journal content".into());
            }
        } else {
            self.records.push(mutation(value));
        }
        self.sync_record(&value["mutation_id"], seq, wire);
        Ok(())
    }
    pub(super) fn advance(&mut self, next: &str) -> Result<()> {
        if sequence(next)? > sequence(&self.cursor()?)? {
            self.state["bindings"][&self.key]["transport_cursor"] = json!(next);
        }
        Ok(())
    }
    pub(super) fn save(&self) -> Result<()> {
        let path = self.dir.join("config.json");
        let config = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let db = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let projected: String = db
            .query_row(
                include_str!("roster-project.sql"),
                params![
                    serde_json::to_string(&self.records).map_err(|e| e.to_string())?,
                    config
                ],
                |r| r.get(0),
            )
            .map_err(|e| format!("cannot project native roster: {e}"))?;
        // Journal first, then derived config, then the independent roster cursor.
        // A retry reprojects the journal and can repeat all three writes safely.
        let mut journal = String::new();
        for record in &self.records {
            journal.push_str(&serde_json::to_string(record).map_err(|e| e.to_string())?);
            journal.push('\n');
        }
        atomic(&self.dir.join("roster.jsonl"), journal.as_bytes())?;
        atomic(&path, projected.as_bytes())?;
        let mut state = serde_json::to_vec(&self.state).map_err(|e| e.to_string())?;
        state.push(b'\n');
        atomic(&self.dir.join("roster-sync.json"), &state)
    }
}

impl Context {
    pub(super) fn reserve_roster(&self, max_blob: usize) -> Result<Vec<Candidate>> {
        let _guard = self.guard()?;
        let mut roster = Roster::load(self)?;
        let candidates = roster.prepare(max_blob)?;
        roster.save()?;
        Ok(candidates)
    }
    pub(super) fn roster_cursor(&self) -> Result<String> {
        let _guard = self.guard()?;
        Roster::load(self)?.cursor()
    }
    pub(super) fn ack_roster(&self, candidate: &Candidate, response: &Value) -> Result<()> {
        let seq = super::ack_sequence(candidate, response)?;
        let _guard = self.guard()?;
        let mut roster = Roster::load(self)?;
        roster.ack(candidate, &seq)?;
        roster.save()
    }
}
