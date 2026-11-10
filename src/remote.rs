//! Bounded JSON reads shared by setup and synchronization.
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::time::Duration;

pub(crate) fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

pub(crate) async fn json(request: RequestBuilder) -> Result<Value, String> {
    // Endpoint paths can contain private capability tokens. Never expose them
    // through reqwest's error strings or redirect to a different server.
    let response = request.send().await.map_err(|_| "remote request failed")?;
    response_json(response).await
}

pub(crate) async fn team_json(
    request: RequestBuilder,
    server: &str,
    team: &str,
) -> Result<Value, String> {
    let value = json(
        request
            .header("agmsg-protocol-version", "1")
            .header("agmsg-team-id", team),
    )
    .await?;
    if value["server_instance_id"] != server
        || value["team_id"] != team
        || value["protocol_version"] != 1
    {
        return Err("remote binding changed; refusing to write or advance cursors".into());
    }
    Ok(value)
}

pub(crate) async fn response_json(mut response: reqwest::Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "remote returned HTTP {}; no cursor was advanced",
            response.status()
        ));
    }
    if response
        .headers()
        .get("agmsg-protocol-version")
        .and_then(|h| h.to_str().ok())
        != Some("1")
    {
        return Err("remote omitted protocol version".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "cannot read remote response")?
    {
        if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
            return Err("remote response exceeds 16 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid remote JSON".into())
}
