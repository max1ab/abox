use super::*;
use rusqlite::Connection;
use tempfile::TempDir;

fn fixture() -> TempDir {
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
        CREATE TABLE messages(id INTEGER PRIMARY KEY AUTOINCREMENT,team TEXT,from_agent TEXT,to_agent TEXT,body TEXT,created_at TEXT);
        CREATE TABLE events(seq INTEGER PRIMARY KEY AUTOINCREMENT,type TEXT,id TEXT,team TEXT,from_agent TEXT,to_agent TEXT,body TEXT,at TEXT,legacy_id INTEGER);").unwrap();
    temp
}
fn seed(ctx: &Context) {
    ctx.store.db.execute("INSERT INTO messages(team,from_agent,to_agent,body,created_at) VALUES ('dev','sender','receiver','中文 @receiver','2026-11-03T00:00:00Z')",[]).unwrap();
}
fn page(candidate: &Candidate, seq: &str) -> Value {
    json!({"messages":[{"id":candidate.id,"envelope":candidate.envelope,"server_seq":seq}],"next_after":seq,"has_more":false})
}
fn incoming() -> Candidate {
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
                })
                .unwrap(),
            ),
        },
    }
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
