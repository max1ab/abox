use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct Profile {
    pub team: String,
    pub agent: String,
    pub display_name: String,
    pub description: String,
    pub updated_at: String,
}

/// agmsg membership enriched with optional profile metadata; not a liveness report.
#[derive(Serialize)]
pub struct Entry {
    pub team: String,
    pub agent: String,
    pub types: Vec<String>,
    pub project: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub team: String,
    pub agent: String,
    pub display_name: String,
    pub description: String,
}
