use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::server::router;

async fn request(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, body)
}

async fn post(app: &Router, team: &str, body: &str) -> Value {
    let (status, saved) = request(
        app.clone(),
        "POST",
        "/v1/board/posts",
        Some(json!({"team": team, "author": "reviewer", "body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    saved
}

#[tokio::test]
async fn posts_preserve_mentions_and_remain_after_repeated_reads_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("board.sqlite");
    let app = router(&db).unwrap();
    let body = "@codex @claude\n结论：下一步审查。\n";
    let saved = post(&app, "dev", body).await;
    assert_eq!(saved["body"], body);
    assert_eq!(saved["author"], "reviewer");
    assert!(saved["created_at"].as_str().unwrap().ends_with('Z'));

    let (_, first) = request(app.clone(), "GET", "/v1/board/posts?team=dev", None).await;
    let (_, second) = request(app.clone(), "GET", "/v1/board/posts?team=dev", None).await;
    assert_eq!(first, json!([saved]));
    assert_eq!(first, second, "board reads must not consume posts");
    drop(app);
    let reopened = router(&db).unwrap();
    let (status, after_restart) = request(reopened, "GET", "/v1/board/posts?team=dev", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first, after_restart);

    // Application storage has no inbox, notification jobs or agmsg tables.
    let connection = Connection::open(&db).unwrap();
    let mut query = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    let tables = query
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(tables, ["agent_profiles", "board_posts"]);
}

#[tokio::test]
async fn team_filter_and_id_pagination_do_not_duplicate_or_skip_posts() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(&temp.path().join("board.sqlite")).unwrap();
    let first = post(&app, "dev", "first").await;
    post(&app, "other", "other team's post").await;
    let second = post(&app, "dev", "second").await;
    let third = post(&app, "dev", "third").await;
    let (_, page) = request(app.clone(), "GET", "/v1/board/posts?team=dev&limit=2", None).await;
    assert_eq!(page, json!([third, second]));

    // A new post between page reads must not shift the cursor-based older page.
    post(&app, "dev", "newer").await;
    let uri = format!("/v1/board/posts?team=dev&limit=2&before={}", second["id"]);
    let (_, older) = request(app.clone(), "GET", &uri, None).await;
    assert_eq!(older, json!([first]));
    let (_, other) = request(app.clone(), "GET", "/v1/board/posts?team=other", None).await;
    assert_eq!(other.as_array().unwrap().len(), 1);
    assert_eq!(other[0]["body"], "other team's post");
    let (_, empty) = request(app, "GET", "/v1/board/posts?team=missing", None).await;
    assert_eq!(empty, json!([]));
}

#[tokio::test]
async fn rejects_invalid_posts_and_pagination_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(&temp.path().join("board.sqlite")).unwrap();
    for body in [
        json!({"team": "dev", "author": "alice", "body": " \n\t"}),
        json!({"team": " dev", "author": "alice", "body": "text"}),
        json!({"team": "dev", "author": "", "body": "text"}),
        json!({"team": "dev", "author": "alice", "body": "x".repeat(MAX_BODY_BYTES + 1)}),
    ] {
        let (status, _) = request(app.clone(), "POST", "/v1/board/posts", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    for query in ["limit=0", "limit=101", "before=0", "before=-1"] {
        let (status, _) = request(
            app.clone(),
            "GET",
            &format!("/v1/board/posts?team=dev&{query}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (_, empty) = request(app, "GET", "/v1/board/posts?team=dev", None).await;
    assert_eq!(empty, json!([]));
}

#[tokio::test]
async fn labels_and_body_are_sql_data() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(&temp.path().join("board.sqlite")).unwrap();
    post(&app, "dev", "normal").await;
    let saved = post(&app, "x'OR'1'='1", "'); DROP TABLE board_posts; --").await;
    let (_, found) = request(
        app.clone(),
        "GET",
        "/v1/board/posts?team=x%27OR%271%27%3D%271",
        None,
    )
    .await;
    assert_eq!(found, json!([saved]));
    let (_, normal) = request(app, "GET", "/v1/board/posts?team=dev", None).await;
    assert_eq!(normal.as_array().unwrap().len(), 1);
    assert_eq!(normal[0]["body"], "normal");
}
