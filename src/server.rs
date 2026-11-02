use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::{board, directory};

#[derive(Serialize, Deserialize)]
pub struct Health {
    pub service: String,
    pub status: String,
}

async fn health(State(store): State<Store>) -> Result<Json<Health>, ApiError> {
    store
        .call(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM board_posts), EXISTS(SELECT 1 FROM agent_profiles)",
                [],
                |_| Ok(()),
            )
        })
        .await?;
    Ok(Json(Health {
        service: "agent-mail".into(),
        status: "ok".into(),
    }))
}

#[derive(Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

pub(crate) struct ApiError(StatusCode, String);

impl ApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, message.into())
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        eprintln!("Agent Mail storage: {error}");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "storage error".into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

#[derive(Clone)]
pub(crate) struct Store(Arc<Mutex<Connection>>);

impl Store {
    fn open(path: &Path) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(|e| e.to_string())?;
        let transaction = connection.transaction().map_err(|e| e.to_string())?;
        board::init_schema(&transaction).map_err(|e| e.to_string())?;
        directory::init_schema(&transaction).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(Self(Arc::new(Mutex::new(connection))))
    }

    pub(crate) async fn call<T: Send + 'static>(
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

pub(crate) fn validate_label(value: &str, field: &str) -> Result<(), ApiError> {
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

pub(crate) fn router(path: &Path) -> Result<Router, String> {
    let store = Store::open(path)?;
    Ok(Router::new()
        .route("/v1/health", get(health))
        .merge(board::routes())
        .merge(directory::routes())
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(store))
}

pub async fn serve(path: &Path, bind: SocketAddr) -> Result<(), String> {
    let app = router(path)?;
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!(
        "Agent Mail listening on http://{}",
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
