use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

struct ApiError(StatusCode, String);

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("board storage: {error}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "board storage error".into(),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

#[derive(Clone)]
struct Store(Arc<Mutex<Connection>>);

impl Store {
    fn open(path: &Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS board_posts (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     team TEXT NOT NULL,
                     author TEXT NOT NULL,
                     body TEXT NOT NULL,
                     created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 );
                 CREATE INDEX IF NOT EXISTS board_posts_team_id ON board_posts(team, id);",
            )
            .map_err(|e| e.to_string())?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    async fn call<T: Send + 'static>(
        self,
        operation: impl FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, ApiError> {
        tokio::task::spawn_blocking(move || {
            let mut connection = self.0.lock().map_err(ApiError::internal)?;
            operation(&mut connection).map_err(ApiError::internal)
        })
        .await
        .map_err(ApiError::internal)?
    }
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

fn validate_label(value: &str, field: &str) -> Result<(), ApiError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > 128
        || value.chars().any(char::is_control)
    {
        return Err(ApiError::bad_request(format!(
            "{field} must be 1–128 characters without surrounding whitespace or control characters"
        )));
    }
    Ok(())
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

fn router(path: &Path) -> Result<Router, String> {
    let store = Store::open(path)?;
    Ok(Router::new()
        .route("/v1/board/posts", get(list).post(publish))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(store))
}

pub async fn serve(path: &Path, bind: SocketAddr) -> Result<(), String> {
    let app = router(path)?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!(
        "Agent Mail board listening on http://{}",
        listener.local_addr().map_err(|e| e.to_string())?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("cannot listen for shutdown: {error}");
            }
        })
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
