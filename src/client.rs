use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use reqwest::{Client, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::board::{MAX_BODY_BYTES, NewPost, Post};
use crate::directory::{Profile, ProfileInput};
use crate::server::ErrorBody;

pub struct ApiClient {
    http: Client,
    board_endpoint: Url,
    profile_endpoint: Url,
}

impl ApiClient {
    pub fn new(server: &str) -> Result<Self, String> {
        let mut base = Url::parse(server).map_err(|e| format!("invalid --server URL: {e}"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("--server must be an HTTP(S) base URL without a query or fragment".into());
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let board_endpoint = base.join("v1/board/posts").map_err(|e| e.to_string())?;
        let profile_endpoint = base
            .join("v1/directory/profiles")
            .map_err(|e| e.to_string())?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            board_endpoint,
            profile_endpoint,
        })
    }

    pub async fn publish(
        &self,
        team: String,
        author: String,
        body: String,
    ) -> Result<Post, String> {
        let response = self
            .http
            .post(self.board_endpoint.clone())
            .json(&NewPost { team, author, body })
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        decode(response).await
    }

    pub async fn list(
        &self,
        team: &str,
        limit: u32,
        before: Option<i64>,
    ) -> Result<Vec<Post>, String> {
        let mut endpoint = self.board_endpoint.clone();
        {
            let mut query = endpoint.query_pairs_mut();
            query
                .append_pair("team", team)
                .append_pair("limit", &limit.to_string());
            if let Some(before) = before {
                query.append_pair("before", &before.to_string());
            }
        }
        let response = self
            .http
            .get(endpoint)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        decode(response).await
    }

    pub async fn set_profile(&self, profile: &ProfileInput) -> Result<Profile, String> {
        let response = self
            .http
            .put(self.profile_endpoint.clone())
            .json(profile)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        decode(response).await
    }

    pub async fn get_profile(&self, team: &str, agent: &str) -> Result<Profile, String> {
        let response = self
            .http
            .get(self.profile_endpoint.clone())
            .query(&[("team", team), ("agent", agent)])
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        decode(response).await
    }
}

async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        let message = response
            .json::<ErrorBody>()
            .await
            .map(|body| body.error)
            .unwrap_or_else(|_| "unexpected server response".into());
        return Err(format!("server returned {status}: {message}"));
    }
    response
        .json()
        .await
        .map_err(|e| format!("invalid server response: {e}"))
}

pub fn read_body(path: Option<&Path>) -> Result<String, String> {
    let input: Box<dyn Read> = match path {
        Some(path) => Box::new(File::open(path).map_err(|e| format!("{}: {e}", path.display()))?),
        None => Box::new(io::stdin()),
    };
    let mut body = String::new();
    input
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|e| format!("cannot read UTF-8 body: {e}"))?;
    if body.trim().is_empty() || body.len() > MAX_BODY_BYTES {
        return Err("body must be nonblank and at most 65536 UTF-8 bytes".into());
    }
    Ok(body)
}

pub fn print_jsonl(items: impl IntoIterator<Item = impl Serialize>) -> Result<(), String> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for item in items {
        serde_json::to_writer(&mut out, &item).map_err(|e| e.to_string())?;
        writeln!(out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_reverse_proxy_prefix_and_rejects_ambiguous_base_urls() {
        let client = ApiClient::new("http://localhost/mail").unwrap();
        assert_eq!(
            client.board_endpoint.as_str(),
            "http://localhost/mail/v1/board/posts"
        );
        assert_eq!(
            client.profile_endpoint.as_str(),
            "http://localhost/mail/v1/directory/profiles"
        );
        for url in [
            "file:///tmp/board",
            "http://localhost/?team=dev",
            "http://localhost/#board",
        ] {
            assert!(ApiClient::new(url).is_err());
        }
    }

    #[test]
    fn file_body_preserves_newlines_and_rejects_blank_oversized_or_invalid_utf8() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("body.txt");
        std::fs::write(&file, "结论\n\n".as_bytes()).unwrap();
        assert_eq!(read_body(Some(&file)).unwrap(), "结论\n\n");
        for body in [
            b" \n\t".to_vec(),
            vec![b'x'; MAX_BODY_BYTES + 1],
            vec![0xff],
        ] {
            std::fs::write(&file, &body).unwrap();
            assert!(read_body(Some(&file)).is_err());
        }
    }
}
