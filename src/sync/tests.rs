use super::*;
use rusqlite::Connection;
use tempfile::TempDir;

pub(super) fn fixture() -> TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("teams/dev")).unwrap();
    fs::create_dir_all(temp.path().join("db")).unwrap();
    fs::write(temp.path().join("VERSION"), "1.7.0\n").unwrap();
    fs::write(temp.path().join("teams/dev/config.json"),serde_json::to_vec(&json!({
        "name":"dev", "agents":{"sender":{"member_id":"019a4000-0000-7000-8000-000000000003","type":"codex"},"receiver":{"member_id":"019a4000-0000-7000-8000-000000000004","type":"codex"}},
        "remote_binding":{"endpoint":"http://127.0.0.1:8787","protocol_version":1,"cipher_profile":"none","disconnected_at":null,
        "server_instance_id":"019a4000-0000-7000-8000-000000000001","remote_team_id":"019a4000-0000-7000-8000-000000000002"}
    })).unwrap()).unwrap();
    let db = Connection::open(temp.path().join("db/messages.db")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA user_version=1;
        CREATE TABLE storage_metadata(key TEXT PRIMARY KEY,value TEXT);
        INSERT INTO storage_metadata VALUES ('read_cursor_v1','1');
        CREATE TABLE messages(id INTEGER PRIMARY KEY AUTOINCREMENT,team TEXT,from_agent TEXT,to_agent TEXT,body TEXT,created_at TEXT,read_at TEXT);
        CREATE TABLE events(seq INTEGER PRIMARY KEY AUTOINCREMENT,type TEXT,id TEXT,team TEXT,from_agent TEXT,to_agent TEXT,body TEXT,at TEXT,legacy_id INTEGER,agent TEXT,msg_id TEXT);
        CREATE TABLE read_cursors(team TEXT,agent TEXT,local_position INTEGER,PRIMARY KEY(team,agent));").unwrap();
    temp
}
fn seed(ctx: &Context) {
    ctx.store.db.execute("INSERT INTO messages(team,from_agent,to_agent,body,created_at) VALUES ('dev','sender','receiver','中文 @receiver','2026-11-03T00:00:00Z')",[]).unwrap();
}
pub(super) fn page(candidate: &Candidate, seq: &str) -> Value {
    json!({"messages":[{"id":candidate.id,"envelope":candidate.envelope,"server_seq":seq}],"next_after":seq,"has_more":false})
}
pub(super) fn incoming() -> Candidate {
    Candidate {
        id: uuid::Uuid::new_v4().to_string(),
        envelope: Envelope {
            v: 1,
            cipher: "none".into(),
            key_id: None,
            blob: STANDARD.encode(
                serde_json::to_vec(&Projection {
                    body: "new private mail".into(),
                    created_at: "2026-11-03T00:00:00.000000Z".into(),
                    from_agent: "sender".into(),
                    to_agent: "receiver".into(),
                    from_member_id: None,
                    to_member_id: None,
                })
                .unwrap(),
            ),
        },
    }
}
#[test]
fn history_boundary_keeps_old_mail_local_and_synchronizes_new_mail() {
    let temp = fixture();
    let path = temp.path().join("teams/dev/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["remote_binding"]["history_boundary"] =
        json!({"local_event":1,"local_message":2,"remote_sequence":2});
    fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut ctx = Context::open(temp.path(), "dev").unwrap();
    seed(&ctx);
    seed(&ctx);
    ctx.store
        .db
        .execute_batch(
            "INSERT INTO events(type,id,team,from_agent,to_agent,body,at,legacy_id)
         SELECT 'message_sent','old-event',team,from_agent,to_agent,body,created_at,id
         FROM messages WHERE id=1;",
        )
        .unwrap();
    assert!(ctx.reserve(1_048_576).unwrap().is_empty());
    seed(&ctx);
    let outgoing = ctx.reserve(1_048_576).unwrap();
    assert_eq!(outgoing.len(), 1);
    let mapped: String = ctx
        .store
        .db
        .query_row("SELECT local_id FROM sync_messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mapped, "3");

    let member = "019a4000-0000-7000-8000-000000000005";
    let roster = Candidate {
        id: uuid::Uuid::new_v4().to_string(),
        envelope: Envelope {
            v: 1,
            cipher: "none".into(),
            key_id: None,
            blob: STANDARD.encode(
                json!({"kind":"member_joined","mutation_id":uuid::Uuid::now_v7().to_string(),
            "member_id":member,"name":"reviewer@B","occurred_at":"2026-11-03T00:00:00.000000Z"})
                .to_string(),
            ),
        },
    };
    assert_eq!(ctx.apply("0", "0", &page(&roster, "1")).unwrap(), 0);
    assert_eq!(ctx.apply("1", "1", &page(&incoming(), "2")).unwrap(), 0);
    assert_eq!(ctx.apply("2", "2", &page(&incoming(), "3")).unwrap(), 1);
    assert_eq!(ctx.cursor().unwrap(), "3");
    assert_eq!(
        crate::registry::config(temp.path(), "dev").unwrap()["agents"]["reviewer@B"]["member_id"],
        member
    );
    let total: i64 = ctx
        .store
        .db
        .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
        .unwrap();
    assert_eq!(total, 4);
}

#[test]
fn reservation_survives_reopen_and_lost_ack_and_self_echo_never_duplicates() {
    let temp = fixture();
    let candidate = {
        let mut ctx = Context::open(temp.path(), "dev").unwrap();
        seed(&ctx);
        ctx.reserve(1_048_576).unwrap().remove(0)
    };
    assert!(uuid(&candidate.id, 4));
    let mut ctx = Context::open(temp.path(), "dev").unwrap();
    let retry = ctx.reserve(1_048_576).unwrap().remove(0);
    assert_eq!(retry.id, candidate.id);
    assert_eq!(retry.envelope, candidate.envelope);
    ctx.ack(
        &retry,
        &json!({"acks":[{"id":retry.id,"server_seq":"1","disposition":"duplicate"}]}),
    )
    .unwrap();
    assert!(ctx.reserve(1_048_576).unwrap().is_empty());
    assert_eq!(ctx.apply("0", "0", &page(&candidate, "1")).unwrap(), 0);
    assert_eq!(ctx.cursor().unwrap(), "1");
    assert_eq!(
        ctx.store
            .db
            .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
#[test]
fn receive_message_mapping_and_cursor_roll_back_together() {
    let temp = fixture();
    let mut ctx = Context::open(temp.path(), "dev").unwrap();
    let candidate = incoming();
    ctx.store.db.execute_batch("CREATE TRIGGER reject_mapping BEFORE INSERT ON sync_messages BEGIN SELECT RAISE(ABORT,'test injected failure'); END;").unwrap();
    assert!(ctx.apply("0", "0", &page(&candidate, "1")).is_err());
    assert_eq!(ctx.cursor().unwrap(), "0");
    assert_eq!(
        ctx.store
            .db
            .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        ctx.store
            .db
            .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    ctx.store
        .db
        .execute_batch("DROP TRIGGER reject_mapping;")
        .unwrap();
    assert_eq!(ctx.apply("0", "0", &page(&candidate, "1")).unwrap(), 1);
    assert_eq!(ctx.cursor().unwrap(), "1");
    assert!(
        ctx.reserve(1_048_576).unwrap().is_empty(),
        "pulled mail must not be uploaded again"
    );
}
#[test]
fn invalid_ack_page_binding_and_sequence_leave_state_unchanged() {
    let temp = fixture();
    let mut ctx = Context::open(temp.path(), "dev").unwrap();
    seed(&ctx);
    let candidate = ctx.reserve(1_048_576).unwrap().remove(0);
    assert!(ctx.ack(&candidate,&json!({"acks":[{"id":uuid::Uuid::new_v4().to_string(),"server_seq":"1","disposition":"stored"}]})).is_err());
    let mut invalid = page(&incoming(), "1");
    invalid["next_after"] = json!("2");
    assert!(ctx.apply("0", "0", &invalid).is_err());
    assert_eq!(ctx.cursor().unwrap(), "0");
    let config_path = temp.path().join("teams/dev/config.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["remote_binding"]["endpoint"] = json!("http://127.0.0.1:9999");
    fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    assert!(
        ctx.apply("0", "0", &page(&incoming(), "1"))
            .unwrap_err()
            .contains("binding changed")
    );
}
#[test]
fn encrypted_and_unknown_roster_pages_are_never_silently_skipped() {
    let temp = fixture();
    let mut ctx = Context::open(temp.path(), "dev").unwrap();
    let mut candidate = incoming();
    candidate.envelope.cipher = "age-v1".into();
    candidate.envelope.key_id = Some("key".into());
    assert!(ctx.apply("0", "0", &page(&candidate, "1")).is_err());
    candidate = incoming();
    candidate.envelope.blob = STANDARD.encode(br#"{"kind":"member_added"}"#);
    assert!(ctx.apply("0", "0", &page(&candidate, "1")).is_err());
    assert_eq!(ctx.cursor().unwrap(), "0");
}
#[test]
fn timestamp_and_endpoint_validation_cover_rollover_and_plaintext_boundary() {
    assert_eq!(
        canonical_time("2024-02-29T12:00:00Z").unwrap(),
        "2024-02-29T12:00:00.000000Z"
    );
    for t in [
        "2023-02-29T00:00:00Z",
        "2026-11-31T00:00:00Z",
        "2026-11-03T24:00:00Z",
        "2026-11-03T00:00:60Z",
    ] {
        assert!(canonical_time(t).is_err());
    }
    for url in [
        "http://8.8.8.8",
        "http://example.com",
        "https://user:pass@example.com",
        "http://localhost/?a=b",
    ] {
        assert!(endpoint(url).is_err());
    }
    for url in [
        "http://127.0.0.1:8787",
        "http://[::1]:8787/mail",
        "https://example.com/mail",
    ] {
        assert!(endpoint(url).is_ok());
    }
}

#[test]
fn roster_updates_preserve_local_registrations_and_never_create_private_mail() {
    let temp = fixture();
    let mut ctx = Context::open(temp.path(), "dev").unwrap();
    let member = "019a4000-0000-7000-8000-000000000005";
    let make = |kind: &str, extra: Value| {
        let mut raw = json!({"kind":kind,"mutation_id":uuid::Uuid::now_v7().to_string(),"member_id":member,"occurred_at":"2026-11-03T00:00:00.000000Z"});
        raw.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        Candidate {
            id: uuid::Uuid::new_v4().to_string(),
            envelope: Envelope {
                v: 1,
                cipher: "none".into(),
                key_id: None,
                blob: STANDARD.encode(serde_json::to_vec(&raw).unwrap()),
            },
        }
    };
    let joined = make("member_joined", json!({"name":"newcomer"}));
    assert_eq!(ctx.apply("0", "0", &page(&joined, "1")).unwrap(), 0);
    let cfg: Value =
        serde_json::from_slice(&fs::read(temp.path().join("teams/dev/config.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["agents"]["newcomer"]["member_id"], member);
    assert_eq!(cfg["agents"]["sender"]["type"], "codex");
    let renamed = make("member_renamed", json!({"from":"newcomer","to":"renamed"}));
    ctx.apply("1", "1", &page(&renamed, "2")).unwrap();
    let left = make("member_left", json!({"name":"renamed"}));
    ctx.apply("2", "2", &page(&left, "3")).unwrap();
    let cfg: Value =
        serde_json::from_slice(&fs::read(temp.path().join("teams/dev/config.json")).unwrap())
            .unwrap();
    assert!(cfg["agents"].get("renamed").is_none());
    assert_eq!(cfg["retired_members"]["renamed"]["member_id"], member);
    assert_eq!(
        ctx.store
            .db
            .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    // Replay because the independent roster cursor was behind: no duplicates,
    // and the already-earned message cursor cannot move backwards.
    ctx.apply("3", "0", &page(&joined, "1")).unwrap();
    assert_eq!(ctx.cursor().unwrap(), "3");
    let journal = fs::read_to_string(temp.path().join("teams/dev/roster.jsonl")).unwrap();
    let id = plain(&joined.envelope).unwrap()["mutation_id"].clone();
    assert_eq!(
        journal
            .lines()
            .filter(|line| serde_json::from_str::<Value>(line).unwrap()["id"] == id)
            .count(),
        1
    );
}

#[test]
fn roster_reservations_survive_reopen_and_lost_ack() {
    let temp = fixture();
    let first = {
        let ctx = Context::open(temp.path(), "dev").unwrap();
        ctx.reserve_roster(1_048_576).unwrap()
    };
    assert_eq!(first.len(), 2);
    let ctx = Context::open(temp.path(), "dev").unwrap();
    let retry = ctx.reserve_roster(1_048_576).unwrap();
    assert_eq!(retry[0].id, first[0].id);
    assert_eq!(retry[0].envelope, first[0].envelope);
    ctx.ack_roster(
        &retry[0],
        &json!({"acks":[{"id":retry[0].id,"server_seq":"1","disposition":"duplicate"}]}),
    )
    .unwrap();
    let remaining = ctx.reserve_roster(1_048_576).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, retry[1].id);
    assert_eq!(ctx.cursor().unwrap(), "0");
}

#[test]
fn reused_remote_address_only_receives_mail_for_the_new_member_id() {
    let sender = fixture();
    let receiver = fixture();
    let sender_id = "019a4000-0000-7000-8000-000000000003";
    let previous_id = "019a4000-0000-7000-8000-000000000004";
    let name = "receiver";
    let project = receiver.path();
    let kind = crate::cli::AgentType::Codex;
    for root in [sender.path(), receiver.path()] {
        let mut config = crate::registry::config(root, "dev").unwrap();
        config["team_id"] = config["remote_binding"]["remote_team_id"].clone();
        fs::write(root.join("teams/dev/config.json"), config.to_string()).unwrap();
    }
    let mut sending = Context::open(sender.path(), "dev").unwrap();
    let enqueue = |db: &Connection, member: &str| {
        db.execute(
            "INSERT INTO events(type,id,team,from_agent,to_agent,body,at)
             VALUES ('message_sent',?1,'dev','sender','receiver',?2,'2026-11-03T00:00:00Z')",
            params![uuid::Uuid::now_v7().to_string(), member],
        )
        .unwrap();
        crate::mailbox::ensure_addresses(db).unwrap();
        crate::mailbox::record_addresses(db, db.last_insert_rowid(), Some(sender_id), Some(member))
            .unwrap();
    };
    enqueue(&sending.store.db, previous_id);
    let delayed = sending.reserve(1_048_576).unwrap().remove(0);
    assert_eq!(
        projection(&delayed.envelope)
            .unwrap()
            .to_member_id
            .as_deref(),
        Some(previous_id)
    );

    let mut receiving = Context::open(receiver.path(), "dev").unwrap();
    let initial = receiving.reserve_roster(1_048_576).unwrap();
    for (index, candidate) in initial.iter().enumerate() {
        let before = index.to_string();
        receiving
            .apply(&before, &before, &page(candidate, &(index + 1).to_string()))
            .unwrap();
    }
    let dir = receiver.path().join("teams/dev");
    let mut journal = fs::OpenOptions::new()
        .append(true)
        .open(dir.join("roster.jsonl"))
        .unwrap();
    use std::io::Write;
    writeln!(journal, "{}", json!({"type":"member_left","id":uuid::Uuid::now_v7().to_string(),"member_id":previous_id,"name":name,"at":"2026-11-03T00:00:00Z"})).unwrap();
    drop(journal);
    crate::registry::join(receiver.path(), "dev", name, kind, project, false).unwrap();
    let config = crate::registry::config(receiver.path(), "dev").unwrap();
    let new_id = config["agents"][name]["member_id"].as_str().unwrap();
    assert_ne!(new_id, previous_id);

    let changes = receiving.reserve_roster(1_048_576).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(plain(&changes[1].envelope).unwrap()["reuse_name"], true);
    for (index, candidate) in initial.iter().chain(&changes).enumerate() {
        let before = index.to_string();
        let after = (index + 1).to_string();
        sending
            .apply(&before, &before, &page(candidate, &after))
            .unwrap();
        if index >= initial.len() {
            receiving
                .apply(&before, &before, &page(candidate, &after))
                .unwrap();
        }
    }
    let sender_config = crate::registry::config(sender.path(), "dev").unwrap();
    assert_eq!(sender_config["agents"][name]["member_id"], new_id);
    assert!(!crate::presence::is_local(&sender_config["agents"][name]));
    assert_eq!(
        sending.reserve(1_048_576).unwrap()[0].envelope,
        delayed.envelope
    );

    receiving.apply("4", "4", &page(&delayed, "5")).unwrap();
    receiving.apply("5", "5", &page(&incoming(), "6")).unwrap();
    assert_eq!(
        crate::mailbox::unread_count(&receiving.store.db, "dev", name, Some(new_id), true).unwrap(),
        0
    );
    enqueue(&sending.store.db, new_id);
    let new_mail = sending.reserve(1_048_576).unwrap().remove(1);
    receiving.apply("6", "6", &page(&new_mail, "7")).unwrap();
    assert_eq!(
        crate::mailbox::unread_count(&receiving.store.db, "dev", name, Some(new_id), true).unwrap(),
        1
    );
    let history = crate::mailbox::history(receiver.path(), "dev", Some(name), 20).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].body, new_id);
}
