//! Durable attempts are reserved before I/O. A lost result never causes an
//! automatic resend. Reading the complete receiving session acknowledges its pending reminder.
use super::Delivery;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, String>;
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
pub(crate) struct State {
    db: Connection,
}
#[derive(Serialize)]
pub(crate) struct Attempt {
    pub nonce: String,
    pub target: String,
    pub state: String,
    pub reason: Option<String>,
    pub receipt: Option<String>,
    pub updated_ms: i64,
}
impl State {
    pub fn open(root: &Path) -> Result<Self> {
        let run = root.join("run");
        fs::create_dir_all(&run).map_err(|e| e.to_string())?;
        let path = run.join("notifications.db");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        drop(options.open(&path).map_err(|e| e.to_string())?);
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        db.pragma_update(None, "journal_mode", "wal")
            .map_err(|e| e.to_string())?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS notification_attempts(nonce TEXT PRIMARY KEY,target TEXT NOT NULL,state TEXT NOT NULL,reason TEXT,receipt TEXT,updated_ms INTEGER NOT NULL,retry_ms INTEGER NOT NULL DEFAULT 0);
        CREATE UNIQUE INDEX IF NOT EXISTS notification_active ON notification_attempts(target) WHERE state IN ('sending','accepted','unknown','failed');").map_err(|e|e.to_string())?;
        Ok(Self { db })
    }
    /// Caller owns the global notification worker lock, so a sending row left
    /// by another process is an interrupted attempt, not concurrent activity.
    pub fn recover(&self) -> Result<()> {
        self.db.execute("UPDATE notification_attempts SET state='unknown',reason='process_interrupted',updated_ms=?1 WHERE state='sending'",[now()]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn reserve(&self, target: &str) -> Result<Option<String>> {
        let nonce = uuid::Uuid::new_v4().to_string();
        let at = now();
        self.db.execute("INSERT OR IGNORE INTO notification_attempts(nonce,target,state,updated_ms) VALUES(?1,?2,'sending',?3)",params![nonce,target,at]).map_err(|e|e.to_string())?;
        if self.db.changes() == 1 {
            return Ok(Some(nonce));
        }
        let retry:Option<String>=self.db.query_row("SELECT nonce FROM notification_attempts WHERE target=?1 AND state='failed' AND retry_ms<=?2",params![target,at],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        if let Some(nonce) = retry {
            self.db.execute("UPDATE notification_attempts SET state='sending',reason=NULL,updated_ms=?2 WHERE nonce=?1 AND state='failed'",params![nonce,at]).map_err(|e|e.to_string())?;
            return Ok(Some(nonce));
        }
        Ok(None)
    }
    pub fn finish(&self, nonce: &str, result: Delivery) -> Result<()> {
        let (state, reason, receipt) = match result {
            Delivery::Accepted { receipt } => ("accepted", None, receipt),
            Delivery::Failed { reason } => ("failed", Some(reason), None),
            Delivery::Unknown { reason } => ("unknown", Some(reason), None),
        };
        self.db.execute("UPDATE notification_attempts SET state=?2,reason=?3,receipt=?4,updated_ms=?5,retry_ms=?6 WHERE nonce=?1 AND state='sending'",params![nonce,state,reason,receipt,now(),now()+30000]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn target(&self, nonce: &str) -> Result<String> {
        self.db
            .query_row(
                "SELECT target FROM notification_attempts WHERE nonce=?1",
                [nonce],
                |r| r.get(0),
            )
            .map_err(|_| "notification nonce is unknown".into())
    }
    pub fn pending(&self, target: &str) -> Result<Option<String>> {
        self.db.query_row(
            "SELECT nonce FROM notification_attempts WHERE target=?1 AND state IN ('sending','accepted','unknown','failed')",
            [target], |r| r.get(0),
        ).optional().map_err(|e| e.to_string())
    }
    pub fn acknowledge(&self, nonce: &str, target: &str) -> Result<()> {
        let changed=self.db.execute("UPDATE notification_attempts SET state='acknowledged',updated_ms=?3 WHERE nonce=?1 AND target=?2",params![nonce,target,now()]).map_err(|e|e.to_string())?;
        if changed == 0 {
            return Err("notification does not belong to this receiving session".into());
        }
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<Attempt>> {
        let mut q=self.db.prepare("SELECT nonce,target,state,reason,receipt,updated_ms FROM notification_attempts ORDER BY updated_ms DESC LIMIT 100").map_err(|e|e.to_string())?;
        q.query_map([], |r| {
            Ok(Attempt {
                nonce: r.get(0)?,
                target: r.get(1)?,
                state: r.get(2)?,
                reason: r.get(3)?,
                receipt: r.get(4)?,
                updated_ms: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<_>>()
        .map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgment_of_a_snapshot_does_not_clear_a_later_or_foreign_reminder() {
        let t = tempfile::tempdir().unwrap();
        let s = State::open(t.path()).unwrap();
        assert!(s.pending("session").unwrap().is_none());
        let first = s.reserve("session").unwrap().unwrap();
        let other = s.reserve("other").unwrap().unwrap();
        let snapshot = s.pending("session").unwrap().unwrap();
        assert_eq!(snapshot, first);
        s.acknowledge(&snapshot, "session").unwrap();
        // An in-flight transport result cannot resurrect a consumed reminder.
        s.finish(&snapshot, Delivery::Accepted { receipt: None })
            .unwrap();
        assert!(s.pending("session").unwrap().is_none());
        let next = s.reserve("session").unwrap().unwrap();
        s.acknowledge(&snapshot, "session").unwrap();
        assert_eq!(
            s.pending("session").unwrap().as_deref(),
            Some(next.as_str())
        );
        assert_eq!(s.pending("other").unwrap().as_deref(), Some(other.as_str()));
    }
    #[test]
    fn reservation_is_coalesced_across_restarts_and_only_acknowledgment_rearms_it() {
        let t = tempfile::tempdir().unwrap();
        let s = State::open(t.path()).unwrap();
        let n = s.reserve("session").unwrap().unwrap();
        drop(s);
        let s = State::open(t.path()).unwrap();
        s.recover().unwrap();
        assert!(s.reserve("session").unwrap().is_none());
        assert_eq!(s.list().unwrap()[0].state, "unknown");
        assert!(s.acknowledge(&n, "other").is_err());
        s.acknowledge(&n, "session").unwrap();
        assert!(s.reserve("session").unwrap().is_some());
    }
}
