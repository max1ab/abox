//! SSE accelerates a cursor-based pull. A reconnect, malformed stream or lost
//! listener never becomes evidence that any mailbox message was delivered.
use super::{Context, Result};
use reqwest::{Client, Url};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

struct Target {
    url: Url,
    server: String,
    team: String,
    config_path: PathBuf,
    binding: Value,
    drivers: Value,
}
impl Target {
    fn load(root: &Path, team: &str) -> Result<Self> {
        let context = Context::open(root, team)?;
        Ok(Self {
            url: context
                .base_url
                .join("v1/agent-mail/events")
                .map_err(|_| "invalid event endpoint")?,
            server: context.server,
            team: context.remote,
            config_path: context.store.team_dir.join("config.json"),
            binding: context.store.config["remote_binding"].clone(),
            drivers: context.store.config["drivers"].clone(),
        })
    }
    fn unchanged(&self) -> bool {
        fs::read(&self.config_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .is_some_and(|config| {
                config["remote_binding"] == self.binding && config["drivers"] == self.drivers
            })
    }
}

#[derive(Default)]
struct Parser {
    pending: Vec<u8>,
    event: String,
    data: Vec<String>,
    size: usize,
}
impl Parser {
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<(String, String)>> {
        let mut frames = Vec::new();
        for &byte in chunk {
            if byte != b'\n' {
                if self.pending.len() >= 16384 {
                    return Err("event stream line exceeds limit".into());
                }
                self.pending.push(byte);
                continue;
            }
            if self.pending.last() == Some(&b'\r') {
                self.pending.pop();
            }
            let line = String::from_utf8(std::mem::take(&mut self.pending))
                .map_err(|_| "event stream is not UTF-8")?;
            if line.is_empty() {
                if !self.data.is_empty() {
                    frames.push((std::mem::take(&mut self.event), self.data.join("\n")));
                }
                self.data.clear();
                self.event.clear();
                self.size = 0;
                continue;
            }
            if line.starts_with(':') {
                continue;
            }
            self.size += line.len();
            if self.size > 16384 {
                return Err("event stream frame exceeds limit".into());
            }
            let (field, value) = line.split_once(':').unwrap_or((&line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event = value.into(),
                "data" => self.data.push(value.into()),
                _ => {}
            }
            if frames.len() > 128 {
                return Err("event stream chunk contains too many frames".into());
            }
        }
        Ok(frames)
    }
}

async fn stream(target: Target, wake: &Notify, mode: &AtomicU8) -> Result<()> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "cannot create event client")?;
    let request = client
        .get(target.url.clone())
        .header("accept", "text/event-stream")
        .header("agmsg-protocol-version", "1")
        .header("agmsg-team-id", &target.team)
        .send();
    let mut response = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .map_err(|_| "event connection timed out")?
        .map_err(|_| "event connection failed")?;
    if !response.status().is_success()
        || response
            .headers()
            .get("content-type")
            .and_then(|s| s.to_str().ok())
            .is_none_or(|s| !s.starts_with("text/event-stream"))
        || response
            .headers()
            .get("agmsg-protocol-version")
            .and_then(|s| s.to_str().ok())
            != Some("1")
    {
        return Err("remote event stream unavailable".into());
    }
    mode.store(1, Ordering::Relaxed);
    wake.notify_one(); // Catch up even when events were missed while disconnected.
    let mut parser = Parser::default();
    let mut check = tokio::time::interval(Duration::from_secs(15));
    let mut deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let chunk = tokio::select! {
            _=check.tick()=>{
                if !target.unchanged() {return Err("event binding changed".into());}
                continue;
            },
            _=tokio::time::sleep_until(deadline)=>return Err("event stream heartbeat timed out".into()),
            chunk=response.chunk()=>chunk.map_err(|_|"event stream read failed")?,
        };
        let Some(chunk) = chunk else {
            return Err("event stream closed".into());
        };
        deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        for (event, data) in parser.feed(&chunk)? {
            if event != "new_messages" {
                continue;
            }
            let value: Value = serde_json::from_str(&data).map_err(|_| "invalid event JSON")?;
            if value["protocol_version"] != 1
                || value["server_instance_id"] != target.server
                || value["team_id"] != target.team
            {
                return Err("event binding does not match connected team".into());
            }
            wake.notify_one();
        }
    }
}

pub(super) async fn follow(root: PathBuf, team: String, wake: Arc<Notify>, mode: Arc<AtomicU8>) {
    let mut delay = 1u64;
    loop {
        let began = std::time::Instant::now();
        if let Ok(target) = Target::load(&root, &team) {
            let _ = stream(target, &wake, &mode).await;
        }
        if mode.swap(2, Ordering::Relaxed) != 2 {
            wake.notify_one();
        }
        if began.elapsed() > Duration::from_secs(20) {
            delay = 1;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(60);
    }
}
pub(crate) fn mode_name(mode: &AtomicU8) -> String {
    match mode.load(Ordering::Relaxed) {
        1 => "connected",
        2 => "fallback",
        _ => "connecting",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_fragmented_utf8_crlf_comments_and_multiline_data() {
        let bytes =
            "event: new_messages\r\ndata: {\"中文\":1,\r\ndata: \"a\":2}\r\n\r\n: keepalive\n\n"
                .as_bytes();
        let mut parser = Parser::default();
        let mut frames = Vec::new();
        for byte in bytes {
            frames.extend(parser.feed(&[*byte]).unwrap());
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "new_messages");
        assert_eq!(
            serde_json::from_str::<Value>(&frames[0].1).unwrap()["中文"],
            1
        );
    }
    #[test]
    fn bounds_lines_and_frames_before_unbounded_buffering() {
        let mut parser = Parser::default();
        assert!(parser.feed(&vec![b'x'; 16385]).is_err());
        let mut parser = Parser::default();
        assert!(parser.feed("data: x\n".repeat(3000).as_bytes()).is_err());
    }
}
