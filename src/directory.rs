use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::server::{ApiError, Store, validate_label};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct Profile {
    pub team: String,
    pub agent: String,
    pub display_name: String,
    pub description: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileInput {
    team: String,
    agent: String,
    display_name: String,
    description: String,
}

#[derive(Deserialize)]
struct ProfileKey {
    team: String,
    agent: String,
}

pub(crate) fn init_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_profiles (
            team TEXT NOT NULL,
            agent TEXT NOT NULL,
            display_name TEXT NOT NULL,
            description TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
            PRIMARY KEY(team, agent)
        );",
    )
}

fn decode_profile(row: &Row<'_>) -> rusqlite::Result<Profile> {
    Ok(Profile {
        team: row.get(0)?,
        agent: row.get(1)?,
        display_name: row.get(2)?,
        description: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

async fn set_profile(
    State(store): State<Store>,
    Json(profile): Json<ProfileInput>,
) -> Result<Json<Profile>, ApiError> {
    validate_label(&profile.team, "team")?;
    validate_label(&profile.agent, "agent")?;
    validate_label(&profile.display_name, "display_name")?;
    if profile.description.len() > 4096 {
        return Err(ApiError::bad_request(
            "description must be at most 4096 UTF-8 bytes",
        ));
    }
    let saved = store
        .call(move |connection| {
            connection.query_row(
                "INSERT INTO agent_profiles(team, agent, display_name, description)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(team, agent) DO UPDATE SET
                 display_name = excluded.display_name,
                 description = excluded.description,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             RETURNING team, agent, display_name, description, updated_at",
                params![
                    profile.team,
                    profile.agent,
                    profile.display_name,
                    profile.description
                ],
                decode_profile,
            )
        })
        .await?;
    Ok(Json(saved))
}

async fn get_profile(
    State(store): State<Store>,
    Query(key): Query<ProfileKey>,
) -> Result<Json<Profile>, ApiError> {
    validate_label(&key.team, "team")?;
    validate_label(&key.agent, "agent")?;
    let profile = store
        .call(move |connection| {
            connection
                .query_row(
                    "SELECT team, agent, display_name, description, updated_at
             FROM agent_profiles WHERE team = ?1 AND agent = ?2",
                    params![key.team, key.agent],
                    decode_profile,
                )
                .optional()
        })
        .await?;
    profile
        .map(Json)
        .ok_or_else(|| ApiError::not_found("profile not found"))
}

pub(crate) fn routes() -> Router<Store> {
    Router::new().route("/v1/directory/profiles", get(get_profile).put(set_profile))
}

#[cfg(test)]
mod tests;
