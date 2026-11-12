use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
#[allow(dead_code)]
mod common;
const SERVER: &str = "019a4000-0000-7000-8000-000000000001";
const TEAM: &str = "019a4000-0000-7000-8000-000000000002";
const MEMBER: &str = "019a4000-0000-7000-8000-000000000003";
const LATER_MEMBER: &str = "019a4000-0000-7000-8000-000000000008";
struct Relay {
    endpoint: String,
    stop: Arc<AtomicBool>,
    fail: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<thread::JoinHandle<()>>,
}
fn common_body() -> Value {
    json!({"protocol_version":1,"server_instance_id":SERVER,"team_id":TEAM,"team_name":"dev","min_available_seq":"0"})
}
fn envelope(id: &str, seq: &str, body: Value) -> Value {
    json!({"id":id,"server_seq":seq,"envelope":{"v":1,"cipher":"none","key_id":null,"blob":STANDARD.encode(body.to_string())}})
}
fn caps(mode: &str) -> Value {
    let mut v = common_body();
    v["current_seq"] = json!(if matches!(mode, "expanded" | "renamed") {
        "3"
    } else {
        "2"
    });
    v["cipher_profile"] = json!(if mode == "encrypted" {
        "age-v1"
    } else {
        "none"
    });
    v["accepted_envelope_versions"] = json!([1]);
    v["write_allowed_ciphers"] = json!(["none"]);
    v["max_blob_bytes"] = json!("1048576");
    if mode == "gap" {
        v["min_available_seq"] = json!("1");
    }
    if mode == "changed" {
        v["server_instance_id"] = json!("019a4000-0000-7000-8000-000000000099");
    }
    v
}
impl Relay {
    fn new(mode: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/prefix", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let fail = Arc::new(AtomicBool::new(mode == "resume"));
        let requests = Arc::new(Mutex::new(vec![]));
        let (shutdown, fail_request, log) = (stop.clone(), fail.clone(), requests.clone());
        let thread = thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buf = [0u8; 1024];
                while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                    let n = match stream.read(&mut buf) {
                        Ok(n) => n,
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            break;
                        }
                        Err(e) => panic!("{e}"),
                    };
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buf[..n]);
                    assert!(bytes.len() < 32768);
                }
                if bytes.is_empty() {
                    continue;
                }
                let request = String::from_utf8(bytes).unwrap();
                let first = request.lines().next().unwrap().to_owned();
                log.lock().unwrap().push(first.clone());
                assert!(request.to_lowercase().contains("agmsg-protocol-version: 1"));
                let path = first.split_whitespace().nth(1).unwrap();
                let (status, body) = if path.starts_with("/prefix/v1/teams?") {
                    let matches = match mode {
                        "missing" => vec![],
                        "ambiguous" => vec![json!({"team_id":TEAM,"team_name":"dev"}); 2],
                        _ => vec![json!({"team_id":TEAM,"team_name":"dev"})],
                    };
                    (
                        200,
                        json!({"protocol_version":1,"server_instance_id":SERVER,"team_name":"dev","teams":matches}),
                    )
                } else if path == "/prefix/v1/agent-mail/teams" && first.starts_with("POST ") {
                    (200, common_body())
                } else if path == format!("/prefix/v1/teams/{TEAM}")
                    || path == "/prefix/v1/capabilities"
                {
                    (200, caps(mode))
                } else if path == "/prefix/v1/members" {
                    let mut value = common_body();
                    let name = if mode == "renamed" {
                        "reviewer@B"
                    } else {
                        "receiver"
                    };
                    value["members"] = json!([{"member_id":MEMBER,"name":name}]);
                    (200, value)
                } else if path.starts_with("/prefix/v1/messages?") {
                    let mut page = common_body();
                    if path.contains("after=0&") {
                        page["messages"] = json!([envelope(
                            "00000000-0000-4000-8000-000000000001",
                            "1",
                            json!({"kind":"member_joined","mutation_id":"019a4000-0000-7000-8000-000000000004","member_id":MEMBER,"name":"receiver","occurred_at":"2026-11-05T00:00:00.000000Z"})
                        )]);
                        page["next_after"] = json!("1");
                        if matches!(mode, "expanded" | "renamed") {
                            let mutation = if mode == "renamed" {
                                json!({"kind":"member_renamed","mutation_id":"019a4000-0000-7000-8000-000000000009",
                                    "member_id":MEMBER,"from":"receiver","to":"reviewer@B","occurred_at":"2026-11-05T00:00:01.000000Z"})
                            } else {
                                json!({"kind":"member_joined","mutation_id":"019a4000-0000-7000-8000-000000000009",
                                    "member_id":LATER_MEMBER,"name":"reviewer@B","occurred_at":"2026-11-05T00:00:01.000000Z"})
                            };
                            page["messages"].as_array_mut().unwrap().push(envelope(
                                "00000000-0000-4000-8000-000000000003",
                                "2",
                                mutation,
                            ));
                            page["next_after"] = json!("2");
                        }
                        page["has_more"] = json!(true);
                        (200, page)
                    } else if fail_request.load(Ordering::SeqCst) {
                        (503, json!({"error":"temporary"}))
                    } else {
                        let seq = if matches!(mode, "expanded" | "renamed") {
                            "3"
                        } else {
                            "2"
                        };
                        page["messages"] = json!([envelope(
                            "00000000-0000-4000-8000-000000000002",
                            seq,
                            json!({"from_agent":"sender","to_agent":"receiver","body":"hello from relay","created_at":"2026-11-05T00:00:00.000000Z"})
                        )]);
                        page["next_after"] = json!(seq);
                        page["has_more"] = json!(false);
                        (200, page)
                    }
                } else if path == "/prefix/v1/agent-mail/events"
                    || (first.starts_with("POST ") && path == "/prefix/v1/messages")
                {
                    (
                        503,
                        json!({"error":"background sync unavailable in fixture"}),
                    )
                } else {
                    panic!("unexpected request {first}");
                };
                let body = body.to_string();
                write!(stream,"HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nAgmsg-Protocol-Version: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        Self {
            endpoint,
            stop,
            fail,
            requests,
            thread: Some(thread),
        }
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn cli(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_abox"));
    common::clean_environment(&mut cmd);
    cmd.env("AGENT_MAIL_BASH", root.join("bash-must-not-run"))
        .env_remove("ABOX_SERVER")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env("CODEX_HOME", root.join("codex-profile"))
        .env("NO_PROXY", "*")
        .current_dir(root)
        .arg("--agmsg-dir")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}
fn fixture() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("VERSION"), "1.7.0\n").unwrap();
    t
}
fn config(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("teams/dev/config.json")).unwrap()).unwrap()
}

#[test]
fn import_accepts_members_added_after_team_creation() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("expanded");
    let imported = cli(
        root,
        &["--server", &relay.endpoint, "team", "import", "dev"],
    );
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let cfg = config(root);
    assert_eq!(cfg["agents"]["receiver"]["member_id"], MEMBER);
    assert_eq!(cfg["agents"]["reviewer@B"]["member_id"], LATER_MEMBER);
    assert!(cfg.get("import_pending").is_none());
}

#[test]
fn import_preserves_identity_across_rename() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("renamed");
    let imported = cli(
        root,
        &["--server", &relay.endpoint, "team", "import", "dev"],
    );
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let cfg = config(root);
    assert!(cfg["agents"].get("receiver").is_none());
    assert_eq!(cfg["agents"]["reviewer@B"]["member_id"], MEMBER);
    assert!(cfg.get("import_pending").is_none());
}

#[test]
fn start_attaches_an_existing_team_without_combining_old_mail() {
    for publish in [false, true] {
        let t = fixture();
        let root = t.path();
        let relay = Relay::new("expanded");
        let local = cli(
            root,
            &[
                "join",
                "dev",
                "--create",
                "--name",
                "reviewer@A",
                "--type",
                "codex",
            ],
        );
        assert!(
            local.status.success(),
            "{}",
            String::from_utf8_lossy(&local.stderr)
        );
        fs::write(root.join("body"), "private local history").unwrap();
        let sent = cli(
            root,
            &[
                "agmsg",
                "send",
                "dev",
                "--from",
                "reviewer@A",
                "--to",
                "reviewer@A",
                "--body-file",
                "body",
            ],
        );
        assert!(
            sent.status.success(),
            "{}",
            String::from_utf8_lossy(&sent.stderr)
        );
        let history = || cli(root, &["history", "--team", "dev"]).stdout;
        let before = history();
        assert!(String::from_utf8_lossy(&before).contains("private local history"));
        let original = config(root);
        let mut args = Vec::from([
            "start",
            "--server",
            &relay.endpoint,
            "--team",
            "dev",
            "--name",
            "reviewer",
            "--machine",
            "A",
            "--type",
            "codex",
            "--session-id",
            "019a4000-0000-7000-8000-000000000005",
            "--no-login",
        ]);
        if publish {
            args.push("--create");
        }
        let started = cli(root, &args);
        let _ = cli(root, &["service", "stop"]);
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let cfg = config(root);
        assert_eq!(cfg["team_id"], original["team_id"]);
        assert_eq!(
            cfg["agents"]["reviewer@A"],
            original["agents"]["reviewer@A"]
        );
        assert_eq!(cfg["agents"]["reviewer@B"]["member_id"], LATER_MEMBER);
        assert_eq!(cfg["remote_binding"]["remote_team_id"], TEAM);
        assert_eq!(
            cfg["remote_binding"]["history_boundary"]["remote_sequence"],
            3
        );
        assert!(cfg.get("attachment_pending").is_none());
        assert_eq!(history(), before);
    }
}

#[test]
fn start_rejects_attachment_with_a_conflicting_full_address() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("expanded");
    let local = cli(
        root,
        &[
            "join",
            "dev",
            "--create",
            "--name",
            "reviewer@B",
            "--type",
            "codex",
        ],
    );
    assert!(
        local.status.success(),
        "{}",
        String::from_utf8_lossy(&local.stderr)
    );
    let before = config(root);
    let started = cli(
        root,
        &[
            "--server",
            &relay.endpoint,
            "start",
            "--team",
            "dev",
            "--name",
            "reviewer@B",
            "--type",
            "codex",
            "--session-id",
            "019a4000-0000-7000-8000-000000000005",
            "--no-login",
        ],
    );
    assert!(!started.status.success());
    let error = String::from_utf8_lossy(&started.stderr);
    assert!(error.contains("address 'reviewer@B'"), "{error}");
    assert_eq!(config(root), before);
}

#[test]
fn start_imports_and_binds_after_a_retry() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("resume");
    let run = || {
        cli(
            root,
            &[
                "start",
                "--server",
                &relay.endpoint,
                "--team",
                "dev",
                "--name",
                "worker",
                "--type",
                "codex",
                "--session-id",
                "019a4000-0000-7000-8000-000000000005",
                "--no-login",
            ],
        )
    };
    let failed = run();
    assert!(!failed.status.success());
    assert_eq!(config(root)["import_pending"], true);
    assert!(config(root)["agents"].get("worker").is_none());

    relay.fail.store(false, Ordering::SeqCst);
    let success = run();
    let stopped = cli(root, &["service", "stop"]);
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let cfg = config(root);
    assert!(cfg.get("import_pending").is_none());
    assert_eq!(cfg["agents"]["receiver"]["member_id"], MEMBER);
    assert!(cfg["agents"]["worker"]["member_id"].is_string());
    let session = fs::read_to_string(root.join("run/role-session.dev__worker")).unwrap();
    assert!(session.contains("019a4000-0000-7000-8000-000000000005"));
}
#[test]
fn paged_import_resumes_without_uploading_or_duplicating_history() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("resume");
    let run = || {
        cli(
            root,
            &["--server", &relay.endpoint, "team", "import", "dev"],
        )
    };
    let first = run();
    assert!(
        !first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(String::from_utf8_lossy(&first.stderr).contains("incomplete"));
    assert_eq!(config(root)["import_pending"], true);
    let blocked = cli(
        root,
        &["join", "dev", "--name", "worker", "--type", "codex"],
    );
    assert!(!blocked.status.success());
    assert!(!cli(root, &["agmsg", "members", "dev"]).status.success());
    relay.fail.store(false, Ordering::SeqCst);
    let retry = run();
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    let cfg = config(root);
    assert!(cfg.get("import_pending").is_none());
    assert_eq!(cfg["agents"]["receiver"]["member_id"], MEMBER);
    assert_eq!(cfg["agents"]["receiver"]["registrations"], json!([]));
    let db = rusqlite::Connection::open(root.join("db/teams/dev/messages.db")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT transport_cursor FROM sync_bindings", [], |r| r
            .get::<_, String>(
            0
        ))
        .unwrap(),
        "2"
    );
    assert!(
        !run().status.success(),
        "completed imports must not overwrite state"
    );
    let requests = relay.requests.lock().unwrap();
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
    assert_eq!(
        requests.iter().filter(|r| r.contains("after=0&")).count(),
        1
    );
}
#[test]
fn lookup_and_policy_failures_do_not_publish_a_local_team() {
    for mode in ["missing", "ambiguous", "encrypted", "gap", "changed"] {
        let t = fixture();
        let relay = Relay::new(mode);
        let out = cli(
            t.path(),
            &["--server", &relay.endpoint, "team", "import", "dev"],
        );
        assert!(!out.status.success(), "{mode}");
        assert!(!t.path().join("teams/dev/config.json").exists(), "{mode}");
        assert!(!t.path().join("db").exists(), "{mode}");
    }
}

#[test]
fn import_refuses_to_merge_an_orphan_database_and_preserves_its_bytes() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("normal");
    let db = root.join("db/teams/dev/messages.db");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(&db, b"existing data").unwrap();
    let output = cli(
        root,
        &["--server", &relay.endpoint, "team", "import", "dev"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("database already exists"));
    assert!(!root.join("teams/dev/config.json").exists());
    assert_eq!(fs::read(db).unwrap(), b"existing data");
}
#[test]
fn pending_import_recovers_when_initial_database_creation_was_interrupted() {
    let t = fixture();
    let root = t.path();
    let relay = Relay::new("resume");
    let run = || {
        cli(
            root,
            &["--server", &relay.endpoint, "team", "import", "dev"],
        )
    };
    assert!(!run().status.success());
    let db = root.join("db/teams/dev/messages.db");
    fs::remove_file(&db).unwrap();
    drop(rusqlite::Connection::open(&db).unwrap());
    relay.fail.store(false, Ordering::SeqCst);
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(config(root).get("import_pending").is_none());
    let db = rusqlite::Connection::open(db).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
