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
