use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};

use crate::server::{ApiError, Store, validate_label};

pub const MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct Post {
    pub id: i64,
    pub team: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewPost {
    pub team: String,
    pub author: String,
    pub body: String,
}

#[derive(Deserialize)]
struct ListQuery {
    team: String,
    #[serde(default = "default_limit")]
    limit: u32,
    before: Option<i64>,
}

fn default_limit() -> u32 {
    20
}

pub(crate) fn init_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS board_posts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            team TEXT NOT NULL,
            author TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
        );
        CREATE INDEX IF NOT EXISTS board_posts_team_id ON board_posts(team, id);",
    )
}

fn decode_post(row: &Row<'_>) -> rusqlite::Result<Post> {
    Ok(Post {
        id: row.get(0)?,
        team: row.get(1)?,
        author: row.get(2)?,
        body: row.get(3)?,
        created_at: row.get(4)?,
    })
}

async fn publish(
    State(store): State<Store>,
    Json(post): Json<NewPost>,
) -> Result<(StatusCode, Json<Post>), ApiError> {
    validate_label(&post.team, "team")?;
    validate_label(&post.author, "author")?;
    if post.body.trim().is_empty() || post.body.len() > MAX_BODY_BYTES {
        return Err(ApiError::bad_request(
            "body must be nonblank and at most 65536 UTF-8 bytes",
        ));
    }
    let saved = store
        .call(move |connection| {
            connection.query_row(
                "INSERT INTO board_posts(team, author, body) VALUES (?1, ?2, ?3)
                 RETURNING id, team, author, body, created_at",
                params![post.team, post.author, post.body],
                decode_post,
            )
        })
        .await?;
    // Publishing has no agmsg dependency or notification path, including for @mentions.
    Ok((StatusCode::CREATED, Json(saved)))
}

async fn list(
    State(store): State<Store>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Post>>, ApiError> {
    validate_label(&query.team, "team")?;
    if !(1..=100).contains(&query.limit) || query.before.is_some_and(|id| id <= 0) {
        return Err(ApiError::bad_request(
            "limit must be 1–100; before must be a positive post ID",
        ));
    }
    let posts = store
        .call(move |connection| {
            let mut statement = connection.prepare(
                "SELECT id, team, author, body, created_at FROM board_posts
                 WHERE team = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
            )?;
            statement
                .query_map(
                    params![query.team, query.before.unwrap_or(i64::MAX), query.limit],
                    decode_post,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    Ok(Json(posts))
}

pub(crate) fn routes() -> Router<Store> {
    Router::new().route("/v1/board/posts", get(list).post(publish))
}

#[cfg(test)]
mod tests;
