//! Local mail queries. All SQL values are bound; no shell or sqlite3 executable.
use std::{fs, io::Write, path::Path};

use rusqlite::{Connection, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub struct Message {
    #[serde(rename = "type")]
    kind: &'static str,
    pub id: String,
    pub team: String,
    pub from: String,
    pub to: String,
    pub body: String,
    pub at: String,
}
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        kind: "message_sent",
        id: row.get(0)?,
        team: row.get(1)?,
        from: row.get(2)?,
        to: row.get(3)?,
        body: row.get(4)?,
        at: row.get(5)?,
    })
}

pub fn teams(root: &Path) -> Result<Vec<Value>, String> {
    let dir = root.join("teams");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().join("config.json").is_file() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "invalid UTF-8 team name")?;
            names.push(name);
        }
    }
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| serde_json::json!({"name":name}))
        .collect())
}

pub fn members(root: &Path, team: &str) -> Result<Vec<crate::agmsg::Member>, String> {
    let config = crate::registry::config(root, team)?;
    if config["import_pending"] == true {
        return Err("team import is incomplete; repeat team import before querying mail".into());
    }
    let agents = config["agents"].as_object().ok_or("invalid team roster")?;
    let mut members = Vec::new();
    for (name, agent) in agents {
        let mut types = Vec::new();
        let registrations = agent["registrations"].as_array();
        let mut project = None;
        if let Some(registrations) = registrations {
            for r in registrations {
                if let Some(kind) = r["type"].as_str()
                    && !types.iter().any(|t| t == kind)
                {
                    types.push(kind.to_owned());
                }
            }
            project = registrations
                .first()
                .and_then(|r| r["project"].as_str())
                .map(str::to_owned);
        } else if let Some(kind) = agent["type"].as_str() {
            types.push(kind.to_owned());
            project = agent["project"].as_str().map(str::to_owned);
        }
        members.push(crate::agmsg::Member {
            name: name.clone(),
            types,
            project,
        });
    }
    members.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(members)
}

pub fn history(
    root: &Path,
    team: &str,
    agent: Option<&str>,
    limit: u32,
) -> Result<Vec<Message>, String> {
    let Some(store) = crate::store::open_existing(root, team)? else {
        return Ok(vec![]);
    };
    if store.config["import_pending"] == true {
        return Err("team import is incomplete; repeat team import before querying mail".into());
    }
    let mut statement = store
        .db
        .prepare(
            "WITH combined AS (
        SELECT id,team,from_agent,to_agent,body,at,seq AS ord FROM events WHERE type='message_sent'
        UNION ALL
        SELECT CAST(id AS TEXT),team,from_agent,to_agent,body,created_at,id FROM messages m
        WHERE NOT EXISTS(SELECT 1 FROM events e WHERE e.legacy_id=m.id)
    ) SELECT id,team,from_agent,to_agent,body,at FROM (
        SELECT * FROM combined WHERE team=?1 AND (?2 IS NULL OR from_agent=?2 OR to_agent=?2)
        ORDER BY ord DESC LIMIT ?3
    ) ORDER BY ord ASC",
        )
        .map_err(|e| e.to_string())?;
    statement
        .query_map(params![team, agent, limit], row)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

pub fn unread(db: &Connection, team: &str, agent: &str) -> Result<Vec<Message>, String> {
    let mut statement = db.prepare("SELECT id,team,from_agent,to_agent,body,at FROM (
        SELECT e.id,e.team,e.from_agent,e.to_agent,e.body,e.at,1 AS src,e.seq AS ord FROM events e
        WHERE e.type='message_sent' AND e.team=?1 AND e.to_agent=?2
          AND e.seq>COALESCE((SELECT local_position FROM read_cursors WHERE team=?1 AND agent=?2),0)
          AND NOT EXISTS(SELECT 1 FROM events r WHERE r.type='message_read' AND r.team=?1 AND r.agent=?2 AND r.msg_id=e.id)
        UNION ALL
        SELECT CAST(m.id AS TEXT),m.team,m.from_agent,m.to_agent,m.body,m.created_at,0,m.id FROM messages m
        WHERE m.team=?1 AND m.to_agent=?2 AND m.read_at IS NULL
          AND NOT EXISTS(SELECT 1 FROM events r WHERE r.type='message_read' AND r.team=?1 AND r.agent=?2 AND r.msg_id=CAST(m.id AS TEXT))
          AND NOT EXISTS(SELECT 1 FROM events e WHERE e.legacy_id=m.id AND e.seq>0)
    ) ORDER BY at,src,ord").map_err(|e| e.to_string())?;
    statement
        .query_map(params![team, agent], row)
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

pub fn inbox(root: &Path, team: &str, agent: &str) -> Result<(), String> {
    crate::store::team_name(agent)?;
    let Some(mut store) = crate::store::open_existing(root, team)? else {
        writeln!(std::io::stdout().lock(), "No new messages.").map_err(|e| e.to_string())?;
        return Ok(());
    };
    if store.config["import_pending"] == true {
        return Err("team import is incomplete; repeat team import before reading mail".into());
    }
    // Snapshot only what is actually displayed. Messages arriving during output
    // must remain unread; output failure must never consume the snapshot.
    let messages = unread(&store.db, team, agent)?;
    let tip: i64 = store
        .db
        .query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='events'),0)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut output = std::io::stdout().lock();
    if messages.is_empty() {
        writeln!(output, "No new messages.").map_err(|e| e.to_string())?;
        return Ok(());
    }
    writeln!(output, "{} new message(s):\n", messages.len()).map_err(|e| e.to_string())?;
    for message in &messages {
        writeln!(
            output,
            "  [{}] {}: {}",
            message.at,
            message.from,
            message.body.replace('\r', "").replace('\n', "\n      ")
        )
        .map_err(|e| e.to_string())?;
    }
    writeln!(output)
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())?;
    consume(&mut store.db, team, agent, &messages, tip)?;
    drop(store);
    crate::sync::runtime::notify(root, team);
    Ok(())
}

fn consume(
    db: &mut Connection,
    team: &str,
    agent: &str,
    messages: &[Message],
    tip: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    for message in messages {
        tx.execute("INSERT INTO events(type,id,team,agent,msg_id,at)
            SELECT 'message_read',?1,?2,?3,?4,strftime('%Y-%m-%dT%H:%M:%SZ','now')
            WHERE NOT EXISTS(SELECT 1 FROM events WHERE type='message_read' AND team=?2 AND agent=?3 AND msg_id=?4)",
            params![uuid::Uuid::now_v7().to_string(),team,agent,message.id]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE messages SET read_at=COALESCE(read_at,strftime('%Y-%m-%dT%H:%M:%SZ','now'))
            WHERE team=?1 AND to_agent=?2 AND (CAST(id AS TEXT)=?3 OR id IN (SELECT legacy_id FROM events WHERE type='message_sent' AND team=?1 AND id=?3))",
            params![team,agent,message.id]).map_err(|e|e.to_string())?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO read_cursors(team,agent,local_position) VALUES(?1,?2,0)",
        params![team, agent],
    )
    .map_err(|e| e.to_string())?;
    tx.execute("UPDATE read_cursors SET local_position=MAX(local_position,COALESCE((
        SELECT MIN(e.seq)-1 FROM events e WHERE e.type='message_sent' AND e.team=?1 AND e.to_agent=?2
          AND e.seq>read_cursors.local_position AND e.seq<=?3
          AND NOT EXISTS(SELECT 1 FROM events r WHERE r.type='message_read' AND r.team=?1 AND r.agent=?2 AND r.msg_id=e.id)
        ),?3)) WHERE team=?1 AND agent=?2",params![team,agent,tip]).map_err(|e|e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consuming_a_snapshot_preserves_mail_arriving_before_marking() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("mail-schema.sql")).unwrap();
        db.execute("INSERT INTO events(type,id,team,from_agent,to_agent,body,at) VALUES('message_sent','first','dev','a','b','first','2026-01-01T00:00:00Z')",[]).unwrap();
        let displayed = unread(&db, "dev", "b").unwrap();
        db.execute("INSERT INTO events(type,id,team,from_agent,to_agent,body,at) VALUES('message_sent','second','dev','a','b','second','2026-01-01T00:00:00Z')",[]).unwrap();
        // Even a high-water observed after the arrival must not consume it.
        consume(&mut db, "dev", "b", &displayed, 2).unwrap();
        assert_eq!(
            unread(&db, "dev", "b")
                .unwrap()
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["second"]
        );
        consume(&mut db, "dev", "b", &displayed, 2).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM events WHERE type='message_read'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn read_mark_failure_rolls_back_mirror_and_cursor() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(include_str!("mail-schema.sql")).unwrap();
        db.execute(
            "INSERT INTO messages(team,from_agent,to_agent,body) VALUES('dev','a','b','legacy')",
            [],
        )
        .unwrap();
        let displayed = unread(&db, "dev", "b").unwrap();
        db.execute_batch("CREATE TRIGGER reject_cursor BEFORE INSERT ON read_cursors BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
        assert!(consume(&mut db, "dev", "b", &displayed, 0).is_err());
        assert_eq!(unread(&db, "dev", "b").unwrap().len(), 1);
        assert_eq!(
            db.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM messages WHERE read_at IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
