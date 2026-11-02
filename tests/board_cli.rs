use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

struct Server {
    child: Child,
    url: String,
}

impl Server {
    fn start(db: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-mail"))
            .args(["serve", "--bind", "127.0.0.1:0", "--db"])
            .arg(db)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(url) = line.strip_prefix("Agent Mail listening on ") {
                    let _ = sender.send(url.to_owned());
                }
            }
        });
        let mut server = Self {
            child,
            url: String::new(),
        };
        server.url = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("board server did not announce a listener within 10 seconds");
        server
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn cli(&self, args: &[&str], body: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agent-mail"))
            .args([
                "--server",
                &self.url,
                "--agmsg-dir",
                "/nonexistent-agmsg-board-test",
                "board",
            ])
            .args(args)
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .stdin(if body.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(body) = body {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(body.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn rows(output: Output) -> Vec<Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
#[ignore = "starts a loopback HTTP listener; run explicitly where local networking is permitted"]
fn separate_cli_processes_share_a_silent_persistent_board_without_agmsg() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("board.sqlite");
    let mut server = Server::start(&db);
    let team = "研发 & review";
    let body = "@codex @claude\n无需通知，只供主动查看。\n";
    let file = temp.path().join("post 'with spaces'.txt");
    std::fs::write(&file, body).unwrap();

    let first = rows(server.cli(
        &[
            "post",
            team,
            "--author",
            "alice",
            "--body-file",
            file.to_str().unwrap(),
        ],
        None,
    ))
    .remove(0);
    assert_eq!(first["body"], body);
    let second = rows(server.cli(
        &["post", team, "--author", "bob", "--stdin"],
        Some("第二条\n"),
    ))
    .remove(0);
    rows(server.cli(
        &["post", "other", "--author", "bob", "--stdin"],
        Some("other team"),
    ));

    let latest = rows(server.cli(&["list", team, "--limit", "1"], None));
    assert_eq!(latest.as_slice(), std::slice::from_ref(&second));
    let before = second["id"].to_string();
    let older = rows(server.cli(&["list", team, "--before", &before], None));
    assert_eq!(older.as_slice(), std::slice::from_ref(&first));
    let all = rows(server.cli(&["list", team], None));
    assert_eq!(all, [second, first]);
    assert_eq!(rows(server.cli(&["list", team], None)), all);
    assert!(rows(server.cli(&["list", "missing"], None)).is_empty());

    let blank = server.cli(&["post", team, "--author", "alice", "--stdin"], Some(" \n"));
    assert_eq!(blank.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&blank.stderr).contains("nonblank"));
    assert_eq!(rows(server.cli(&["list", team], None)), all);

    // An abrupt process exit must not lose posts that were already acknowledged.
    server.stop();
    let reopened = Server::start(&db);
    assert_eq!(rows(reopened.cli(&["list", team], None)), all);
}

#[tokio::test]
#[ignore = "starts a loopback HTTP listener; run explicitly where local networking is permitted"]
async fn directory_profile_api_shares_the_service_and_persists_updates() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("mail.sqlite");
    let mut server = Server::start(&db);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let endpoint = format!("{}/v1/directory/profiles", server.url);
    let mut body = serde_json::json!({
        "team": "dev", "agent": "reviewer", "display_name": "审查员", "description": "初始简介"
    });
    for description in ["初始简介", "更新后的简介"] {
        body["description"] = serde_json::json!(description);
        let response = http.put(&endpoint).json(&body).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap()["description"],
            description
        );
    }
    server.stop();
    let reopened = Server::start(&db);
    let response = http
        .get(format!("{}/v1/directory/profiles", reopened.url))
        .query(&[("team", "dev"), ("agent", "reviewer")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["description"],
        "更新后的简介"
    );
    let missing = http
        .get(format!("{}/v1/directory/profiles", reopened.url))
        .query(&[("team", "other"), ("agent", "reviewer")])
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
}
