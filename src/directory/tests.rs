use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::server::router;

use super::*;

async fn request(app: Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
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

fn input(team: &str, agent: &str) -> Value {
    json!({"team": team, "agent": agent, "display_name": "审查员", "description": "负责代码审查"})
}

async fn save(app: &Router, body: Value) -> Value {
    let (status, saved) = request(app.clone(), "PUT", "/v1/directory/profiles", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    saved
}

#[tokio::test]
async fn updates_same_role_preserves_other_roles_and_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("mail.sqlite");
    let app = router(&db).unwrap();
    save(&app, input("dev", "reviewer")).await;
    let teammate = save(&app, input("dev", "reviewer2")).await;
    let other_team = save(&app, input("other", "reviewer")).await;
    let mut changed = input("dev", "reviewer");
    changed["description"] = json!(""); // Empty description explicitly clears it.
    let saved = save(&app, changed).await;
    assert_eq!(saved["description"], "");
    assert_eq!(
        saved["display_name"], teammate["display_name"],
        "display names may repeat"
    );
    assert!(saved["updated_at"].as_str().unwrap().ends_with('Z'));
    drop(app);

    let reopened = router(&db).unwrap();
    for (query, expected) in [
        ("team=dev&agent=reviewer", saved),
        ("team=dev&agent=reviewer2", teammate),
        ("team=other&agent=reviewer", other_team),
    ] {
        let uri = format!("/v1/directory/profiles?{query}");
        for _ in 0..2 {
            let (status, profile) = request(reopened.clone(), "GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(profile, expected, "reading must not change profile data");
        }
    }
    let connection = Connection::open(&db).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM agent_profiles", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 3, "updating a role must not duplicate it");
}

#[tokio::test]
async fn missing_profile_returns_404_without_registering_a_role() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(&temp.path().join("mail.sqlite")).unwrap();
    let (status, error) = request(
        app,
        "GET",
        "/v1/directory/profiles?team=dev&agent=missing",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"], "profile not found");
}

#[tokio::test]
async fn invalid_update_does_not_replace_valid_profile() {
    let temp = tempfile::tempdir().unwrap();
    let app = router(&temp.path().join("mail.sqlite")).unwrap();
    let original = save(&app, input("dev", "reviewer")).await;
    for (field, value) in [
        ("team", json!(" dev")),
        ("agent", json!("")),
        ("display_name", json!("\n")),
        ("display_name", json!("x".repeat(129))),
        ("description", json!("字".repeat(1366))),
    ] {
        let mut body = input("dev", "reviewer");
        body[field] = value;
        let (status, _) = request(app.clone(), "PUT", "/v1/directory/profiles", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (_, retained) = request(
        app,
        "GET",
        "/v1/directory/profiles?team=dev&agent=reviewer",
        None,
    )
    .await;
    assert_eq!(retained, original);
}

#[tokio::test]
async fn adding_profiles_to_an_existing_board_database_keeps_posts() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("mail.sqlite");
    {
        let connection = Connection::open(&db).unwrap();
        crate::board::init_schema(&connection).unwrap();
        connection.execute("INSERT INTO board_posts(team, author, body) VALUES ('dev', 'alice', '@reviewer existing post')", []).unwrap();
    }
    let app = router(&db).unwrap();
    let key = "reviewer'1";
    let saved = save(&app, input("dev", key)).await;
    let (_, profile) = request(
        app.clone(),
        "GET",
        "/v1/directory/profiles?team=dev&agent=reviewer%271",
        None,
    )
    .await;
    assert_eq!(profile, saved);
    let (status, posts) = request(app, "GET", "/v1/board/posts?team=dev", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(posts.as_array().unwrap().len(), 1);
    assert_eq!(posts[0]["body"], "@reviewer existing post");
}
