-- Existing mailbox format; retained while the notification service is replaced.
BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS events (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, type TEXT NOT NULL, id TEXT NOT NULL,
 team TEXT, from_agent TEXT, to_agent TEXT, body TEXT, msg_id TEXT, agent TEXT,
 at TEXT NOT NULL, legacy_id INTEGER
);
CREATE INDEX IF NOT EXISTS events_sent ON events(type,team,to_agent,seq);
CREATE INDEX IF NOT EXISTS events_read ON events(type,team,agent,msg_id);
CREATE INDEX IF NOT EXISTS events_legacy ON events(legacy_id);
CREATE INDEX IF NOT EXISTS events_id ON events(id);
CREATE TABLE IF NOT EXISTS messages (
 id INTEGER PRIMARY KEY AUTOINCREMENT, team TEXT NOT NULL, from_agent TEXT NOT NULL,
 to_agent TEXT NOT NULL, body TEXT NOT NULL,
 created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%SZ','now')), read_at TEXT
);
CREATE TABLE IF NOT EXISTS read_cursors (
 team TEXT NOT NULL, agent TEXT NOT NULL,
 local_position INTEGER NOT NULL DEFAULT 0 CHECK(local_position>=0), PRIMARY KEY(team,agent)
);
CREATE TABLE IF NOT EXISTS storage_metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL);
INSERT OR IGNORE INTO storage_metadata(key,value) VALUES('read_cursor_v1','1');
PRAGMA user_version=1;
COMMIT;
