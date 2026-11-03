use std::fs::OpenOptions;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::{board::Post, directory::Profile};

#[derive(Serialize)]
struct Snapshot {
    version: u32,
    posts: Vec<Post>,
    profiles: Vec<Profile>,
}

/// Read a consistent snapshot without initializing or modifying the old database.
/// Stop its writer before export and keep it stopped until cutover is verified.
pub fn export(db: &Path, output: &Path) -> Result<(), String> {
    let mut connection = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    let tx = connection.transaction().map_err(|e| e.to_string())?;
    let snapshot = (|| -> rusqlite::Result<Snapshot> {
        let posts = tx.prepare("SELECT id,team,author,body,created_at FROM board_posts ORDER BY id")?
            .query_map([], |row| Ok(Post { id: row.get(0)?, team: row.get(1)?, author: row.get(2)?, body: row.get(3)?, created_at: row.get(4)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let profiles = tx.prepare("SELECT team,agent,display_name,description,updated_at FROM agent_profiles ORDER BY team,agent")?
            .query_map([], |row| Ok(Profile { team: row.get(0)?, agent: row.get(1)?, display_name: row.get(2)?, description: row.get(3)?, updated_at: row.get(4)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Snapshot { version: 1, posts, profiles })
    })().map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(file, &snapshot).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_ids_text_and_timestamps_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("old.sqlite3");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE board_posts(id INTEGER PRIMARY KEY AUTOINCREMENT,team TEXT,author TEXT,body TEXT,created_at TEXT);
            CREATE TABLE agent_profiles(team TEXT,agent TEXT,display_name TEXT,description TEXT,updated_at TEXT,PRIMARY KEY(team,agent));").unwrap();
        conn.execute("INSERT INTO board_posts(id,team,author,body,created_at) VALUES (7,'t','a','中文 @dell','original')", []).unwrap();
        conn.execute(
            "INSERT INTO agent_profiles VALUES ('t','a','名字','描述','original')",
            [],
        )
        .unwrap();
        drop(conn);
        let output = temp.path().join("snapshot.json");
        export(&db, &output).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(saved["posts"][0]["id"], 7);
        assert_eq!(saved["posts"][0]["body"], "中文 @dell");
        assert_eq!(saved["profiles"][0]["updated_at"], "original");
        assert!(export(&db, &output).is_err());
        let absent = temp.path().join("absent.sqlite3");
        assert!(export(&absent, &temp.path().join("bad.json")).is_err());
        assert!(!absent.exists());
    }
}
