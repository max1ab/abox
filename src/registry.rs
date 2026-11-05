//! Native team registry. Retains the existing on-disk journal and local registrations.
use crate::{cli::AgentType, store::TeamLock};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

type Result<T> = std::result::Result<T, String>;

pub fn config(root: &Path, team: &str) -> Result<Value> {
    crate::store::team_name(team)?;
    let path = root.join("teams").join(team).join("config.json");
    let bytes = fs::read(&path).map_err(|e| format!("cannot read team '{team}': {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid team config: {e}"))
}

pub fn now() -> Result<String> {
    Connection::open_in_memory()
        .and_then(|db| {
            db.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| {
                r.get(0)
            })
        })
        .map_err(|e| e.to_string())
}

pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn records(dir: &Path, cfg: &Value) -> Result<Vec<Value>> {
    match fs::read_to_string(dir.join("roster.jsonl")) {
        Ok(text) => text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| format!("invalid roster journal: {e}")))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let at = now()?;
            let agents = cfg["agents"].as_object().ok_or("invalid team roster")?;
            agents.iter().map(|(name, agent)| {
                let member = agent["member_id"].as_str().ok_or("team contains an identity without a member ID; repair before native registration")?;
                Ok(json!({"type":"member_joined","id":uuid::Uuid::now_v7().to_string(),"member_id":member,"name":name,"at":at}))
            }).collect()
        }
        Err(e) => Err(e.to_string()),
    }
}
fn project(records: &[Value], cfg: &Value) -> Result<Value> {
    crate::sync::validate_journal(records)?;
    let db = Connection::open_in_memory().map_err(|e| e.to_string())?;
    let text: String = db
        .query_row(
            include_str!("sync/roster-project.sql"),
            params![
                serde_json::to_string(records).map_err(|e| e.to_string())?,
                cfg.to_string()
            ],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}
fn save(dir: &Path, records: &[Value], cfg: &Value) -> Result<()> {
    let mut journal = String::new();
    for record in records {
        journal.push_str(&record.to_string());
        journal.push('\n');
    }
    // Keep the identity journal authoritative; config carries local registrations.
    atomic(&dir.join("roster.jsonl"), journal.as_bytes())?;
    atomic(&dir.join("config.json"), cfg.to_string().as_bytes())
}

pub fn registered(cfg: &Value, name: &str, kind: AgentType, path: &Path) -> bool {
    let member = &cfg["agents"][name];
    let matches = |r: &Value| {
        r["type"] == kind.as_str()
            && r["project"].as_str().is_some_and(|p| {
                crate::store::native_path(Path::new(p))
                    .canonicalize()
                    .is_ok_and(|p| p == path)
            })
    };
    member["registrations"]
        .as_array()
        .map_or_else(|| matches(member), |rs| rs.iter().any(matches))
}

pub fn join(
    root: &Path,
    team: &str,
    name: &str,
    kind: AgentType,
    path: &Path,
    create: bool,
) -> Result<()> {
    crate::store::team_name(team)?;
    crate::store::team_name(name)?;
    if team.chars().count() > 128 || name.chars().count() > 128 {
        return Err("team and identity names must be at most 128 characters".into());
    }
    let path = path
        .canonicalize()
        .map_err(|e| format!("cannot open project: {e}"))?;
    if !path.is_dir() {
        return Err("project must be a directory".into());
    }
    let dir = root.join("teams").join(team);
    if !dir.join("config.json").exists() && !create {
        return Err("team not found locally; run team import first, or use join --create for a new local team".into());
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _lock = TeamLock::acquire(&dir)?;
    let mut cfg = if dir.join("config.json").exists() {
        config(root, team)?
    } else {
        json!({"name":team,"team_id":uuid::Uuid::now_v7().to_string(),"agents":{},"created_at":now()?})
    };
    if cfg["import_pending"] == true {
        return Err("team import is incomplete; repeat team import before joining".into());
    }
    if !cfg.is_object() || !cfg["agents"].is_object() {
        return Err("invalid team roster".into());
    }
    if cfg["team_id"].as_str().is_none() {
        return Err(
            "team has no stable ID; native registration requires an identity-bearing team".into(),
        );
    }
    let mut journal = records(&dir, &cfg)?;
    cfg = project(&journal, &cfg)?;
    if registered(&cfg, name, kind, &path) {
        crate::store::initialize(root, team, &cfg)?;
        save(&dir, &journal, &cfg)?;
        println!("Already registered as {team}/{name}; use bind for this session.");
        return Ok(());
    }
    if cfg["agents"].get(name).is_some_and(|v| !v.is_null()) {
        return Err("name already belongs to another registration; choose a different name".into());
    }
    // Names remain attached to their original identity even after rename/leave.
    if journal
        .iter()
        .any(|r| r["name"] == name || r["from"] == name || r["to"] == name)
        || cfg["renamed"]
            .as_array()
            .is_some_and(|rs| rs.iter().any(|r| r["from"] == name))
    {
        return Err(
            "name belongs to a retired or renamed identity; choose a different name".into(),
        );
    }
    let member = uuid::Uuid::now_v7().to_string();
    journal.push(json!({"type":"member_joined","id":uuid::Uuid::now_v7().to_string(),"member_id":member,"name":name,"at":now()?}));
    cfg["agents"][name] =
        json!({"member_id":member,"registrations":[{"type":kind.as_str(),"project":path}]});
    cfg = project(&journal, &cfg)?;
    crate::store::initialize(root, team, &cfg)?;
    save(&dir, &journal, &cfg)?;
    println!("Registered {team}/{name}; run bind inside the receiving session.");
    crate::sync::runtime::notify(root, team);
    Ok(())
}
