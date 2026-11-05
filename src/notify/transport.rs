use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub(crate) enum Target {
    Codex {
        session: String,
        profile: PathBuf,
    },
    Claude {
        session: String,
        profile: PathBuf,
        project: PathBuf,
        socket: PathBuf,
        token: String,
    },
}
impl Target {
    pub fn from_record(record: &BTreeMap<String, String>) -> Result<Self, String> {
        let get = |key: &str| {
            record
                .get(key)
                .filter(|s| !s.is_empty())
                .cloned()
                .ok_or_else(|| format!("missing session field: {key}"))
        };
        let session = get("session")?;
        if !uuid::Uuid::parse_str(&session).is_ok_and(|id| id.to_string() == session) {
            return Err("invalid session UUID".into());
        }
        let path = |key: &str| -> Result<PathBuf, String> {
            let raw = get(key)?;
            let p = crate::store::native_path(Path::new(&raw));
            if !p.is_absolute() || raw.chars().any(char::is_control) {
                return Err(format!("invalid session path: {key}"));
            }
            Ok(p)
        };
        match get("type")?.as_str() {
            "codex" => Ok(Self::Codex {
                session,
                profile: path("codex_home")?,
            }),
            "claude-code" => {
                if record
                    .get("messaging_blocked")
                    .is_some_and(|s| !s.is_empty())
                {
                    return Err("Claude messaging is blocked; refresh the session binding".into());
                }
                let token = record.get("messaging_token").cloned().unwrap_or_default();
                if !token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                {
                    return Err("invalid Claude messaging token".into());
                }
                Ok(Self::Claude {
                    session,
                    profile: path("claude_config_dir")?,
                    project: path("project")?,
                    socket: path("messaging_socket")?,
                    token,
                })
            }
            _ => Err("unsupported notification agent type".into()),
        }
    }
    pub fn session(&self) -> &str {
        match self {
            Self::Codex { session, .. } | Self::Claude { session, .. } => session,
        }
    }
    pub fn agent_type(&self) -> &str {
        match self {
            Self::Codex { .. } => "codex",
            Self::Claude { .. } => "claude-code",
        }
    }
    /// A receiving process/profile, not a team. Several mailboxes share one reminder.
    pub fn key(&self) -> String {
        let value = match self {
            Self::Codex { session, profile } => json!(["codex", profile, session]),
            Self::Claude {
                session,
                profile,
                socket,
                ..
            } => json!(["claude-code", profile, session, socket]),
        };
        ring::digest::digest(&ring::digest::SHA256, value.to_string().as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Delivery {
    Accepted { receipt: Option<String> },
    Failed { reason: String },
    Unknown { reason: String },
}
fn failed(reason: &str) -> Delivery {
    Delivery::Failed {
        reason: reason.into(),
    }
}
fn unknown(reason: &str) -> Delivery {
    Delivery::Unknown {
        reason: reason.into(),
    }
}

pub(crate) async fn deliver(
    target: &Target,
    message: &str,
    nonce: &str,
    timeout: Duration,
) -> Delivery {
    if uuid::Uuid::parse_str(nonce).is_err() || message.is_empty() {
        return failed("invalid_notification");
    }
    match target {
        Target::Codex { session, profile } => codex(session, profile, message, timeout).await,
        Target::Claude { socket, token, .. } => {
            claude(target, socket, token, message, nonce, timeout).await
        }
    }
}
async fn codex(session: &str, profile: &Path, message: &str, timeout: Duration) -> Delivery {
    let executable = std::env::var_os("AGENT_MAIL_CODEX").unwrap_or_else(|| "codex".into());
    codex_with(&executable, session, profile, message, timeout).await
}
async fn codex_with(
    executable: &std::ffi::OsStr,
    session: &str,
    profile: &Path,
    message: &str,
    timeout: Duration,
) -> Delivery {
    let mut cmd = tokio::process::Command::new(executable);
    cmd.args(["queue", "--thread", session, "--message", message])
        .env("CODEX_HOME", profile)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return failed("codex_spawn_failed"),
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let attempt = async {
        let out = async {
            let mut b = Vec::new();
            stdout.take(65537).read_to_end(&mut b).await.map(|_| b)
        };
        let err = async {
            let mut b = Vec::new();
            stderr.take(65537).read_to_end(&mut b).await.map(|_| b)
        };
        tokio::join!(child.wait(), out, err)
    };
    let (status, out, err) = match tokio::time::timeout(timeout, attempt).await {
        Ok(result) => result,
        Err(_) => {
            let _ = child.start_kill();
            return unknown("codex_timeout");
        }
    };
    let (Ok(status), Ok(out), Ok(err)) = (status, out, err) else {
        return unknown("codex_io_failed");
    };
    if out.len() > 65536 || err.len() > 65536 {
        return unknown("codex_output_limit");
    }
    classify_queue(
        status.success(),
        &String::from_utf8_lossy(&out),
        &String::from_utf8_lossy(&err),
        session,
    )
}
fn classify_queue(success: bool, out: &str, err: &str, session: &str) -> Delivery {
    if !success {
        if err.contains("code -32600") {
            return failed("codex_thread_archived");
        }
        if err.contains("code -32603") {
            return failed("codex_rollout_missing");
        }
        // A nonzero exit can happen after enqueue: don't blindly duplicate it.
        return unknown("codex_queue_failed");
    }
    for line in out.lines() {
        let parts: Vec<_> = line
            .trim()
            .trim_end_matches('.')
            .split_whitespace()
            .collect();
        if let ["Queued", "message", receipt, "for", "thread", thread] = parts.as_slice()
            && *thread == session
            && uuid::Uuid::parse_str(receipt).is_ok()
        {
            return Delivery::Accepted {
                receipt: Some((*receipt).into()),
            };
        }
    }
    unknown("codex_output_unrecognized")
}
fn bypass(target: &Target) -> bool {
    let Target::Claude {
        session,
        profile,
        project,
        ..
    } = target
    else {
        return false;
    };
    let cwd = project.to_string_lossy();
    let encoded: String = cwd
        .chars()
        .map(|c| {
            if cfg!(windows) {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            } else if c == '/' {
                '-'
            } else {
                c
            }
        })
        .collect();
    let path = profile
        .join("projects")
        .join(encoded)
        .join(format!("{session}.jsonl"));
    let read = || -> std::io::Result<bool> {
        let mut f = fs::File::open(path)?;
        let size = f.metadata()?.len();
        f.seek(SeekFrom::Start(size.saturating_sub(1024 * 1024)))?;
        let mut text = String::new();
        f.take(1024 * 1024).read_to_string(&mut text)?;
        for line in text.lines().rev() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                && let Some(mode) = v["permissionMode"].as_str()
            {
                return Ok(mode == "bypassPermissions");
            }
        }
        Ok(false)
    };
    read().unwrap_or(false)
}
fn wire(target: &Target, token: &str, message: &str, nonce: &str) -> Vec<u8> {
    // Keep the established native-wire sender label for compatibility; this
    // is an on-wire string, not a dependency on the external agmsgd process.
    let mode = if bypass(target) {
        " from-name=\"agmsgd\" from-mode=\"bypass\""
    } else {
        ""
    };
    let payload = json!({"msgV":1,"msg_id":nonce,"type":"user","message":{"role":"user","content":format!("<cross-session-message from=\"agmsgd\"{mode}>\n{message}\n</cross-session-message>")},"priority":"next","from":"agmsgd"});
    let auth = if token.is_empty() {
        String::new()
    } else {
        format!("{}\n", json!({"type":"auth","token":token}))
    };
    format!("{auth}{payload}\n").into_bytes()
}
async fn write<S: tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    bytes: &[u8],
    timeout: Duration,
) -> Delivery {
    let mut position = 0;
    let attempt = async {
        while position < bytes.len() {
            let n = stream.write(&bytes[position..]).await?;
            if n == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            position += n;
        }
        stream.shutdown().await
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(Ok(())) => Delivery::Accepted { receipt: None },
        _ if position == 0 => failed("claude_socket_write_failed"),
        _ => unknown("claude_socket_partial_write"),
    }
}
async fn claude(
    target: &Target,
    socket: &Path,
    token: &str,
    message: &str,
    nonce: &str,
    timeout: Duration,
) -> Delivery {
    let bytes = wire(target, token, message, nonce);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let metadata = match fs::symlink_metadata(socket) {
            Ok(m) => m,
            Err(_) => return failed("claude_socket_missing"),
        };
        // geteuid is a read-only libc call and has no preconditions.
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return failed("claude_socket_not_owned");
        }
        let mut stream =
            match tokio::time::timeout(timeout, tokio::net::UnixStream::connect(socket)).await {
                Ok(Ok(s)) => s,
                _ => return failed("claude_socket_connect_failed"),
            };
        write(&mut stream, &bytes, timeout).await
    }
    #[cfg(windows)]
    {
        if !socket
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(r"\\.\pipe\")
        {
            return failed("claude_pipe_must_be_local");
        }
        let mut stream = match tokio::net::windows::named_pipe::ClientOptions::new()
            .read(false)
            .write(true)
            .open(socket)
        {
            Ok(s) => s,
            Err(_) => return failed("claude_pipe_connect_failed"),
        };
        write(&mut stream, &bytes, timeout).await
    }
    #[cfg(not(any(unix, windows)))]
    failed("claude_socket_unsupported")
}

#[cfg(test)]
mod tests {
    use super::*;
    const SESSION: &str = "019a4000-0000-7000-8000-000000000001";
    #[cfg(unix)]
    #[tokio::test]
    async fn codex_process_uses_literal_argv_and_a_bounded_timeout() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("fake-codex");
        fs::write(&exe,"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CODEX_HOME/args\"\nprintf 'Queued message 00000000-0000-4000-8000-000000000001 for thread %s.\\n' \"$3\"\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let body = "$(never-run) 'quote'\n中文";
        let result = codex_with(
            exe.as_os_str(),
            SESSION,
            t.path(),
            body,
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(result, Delivery::Accepted { .. }));
        assert_eq!(
            fs::read_to_string(t.path().join("args")).unwrap(),
            format!("queue\n--thread\n{SESSION}\n--message\n{body}\n")
        );
        fs::write(&exe, "#!/bin/sh\nexec sleep 2\n").unwrap();
        assert!(matches!(
            codex_with(
                exe.as_os_str(),
                SESSION,
                t.path(),
                body,
                Duration::from_millis(20)
            )
            .await,
            Delivery::Unknown { .. }
        ));
    }
    #[test]
    fn queue_acceptance_requires_a_receipt_for_the_requested_thread() {
        assert!(matches!(
            classify_queue(
                true,
                &format!(
                    "Queued message 00000000-0000-4000-8000-000000000001 for thread {SESSION}.\n"
                ),
                "",
                SESSION
            ),
            Delivery::Accepted { .. }
        ));
        assert!(matches!(
            classify_queue(true, "success", "", SESSION),
            Delivery::Unknown { .. }
        ));
        assert!(matches!(
            classify_queue(false, "", "code -32600", SESSION),
            Delivery::Failed { .. }
        ));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn socket_receives_auth_then_the_original_native_wire_format() {
        let t = tempfile::tempdir().unwrap();
        let socket = t.path().join("socket");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let target = Target::Claude {
            session: SESSION.into(),
            profile: t.path().into(),
            project: t.path().into(),
            socket,
            token: "secret".into(),
        };
        let nonce = uuid::Uuid::new_v4().to_string();
        let (result, bytes) = tokio::join!(
            deliver(&target, "check inbox", &nonce, Duration::from_secs(1)),
            async {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut b = Vec::new();
                s.read_to_end(&mut b).await.unwrap();
                b
            }
        );
        assert!(matches!(result, Delivery::Accepted { .. }));
        let text = String::from_utf8(bytes).unwrap();
        let rows: Vec<_> = text
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .collect();
        assert_eq!(rows[0]["token"], "secret");
        assert_eq!(rows[1]["msg_id"], nonce);
        assert_eq!(rows[1]["priority"], "next");
        assert!(
            rows[1]["message"]["content"]
                .as_str()
                .unwrap()
                .contains("check inbox")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn regular_files_and_symlinks_are_not_messaging_sockets() {
        let t = tempfile::tempdir().unwrap();
        let socket = t.path().join("file");
        fs::write(&socket, "unchanged").unwrap();
        let mut target = Target::Claude {
            session: SESSION.into(),
            profile: t.path().into(),
            project: t.path().into(),
            socket: socket.clone(),
            token: String::new(),
        };
        assert!(matches!(
            deliver(
                &target,
                "hello",
                &uuid::Uuid::new_v4().to_string(),
                Duration::from_secs(1)
            )
            .await,
            Delivery::Failed { .. }
        ));
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&socket, &link).unwrap();
        if let Target::Claude { socket, .. } = &mut target {
            *socket = link;
        }
        assert!(matches!(
            deliver(
                &target,
                "hello",
                &uuid::Uuid::new_v4().to_string(),
                Duration::from_secs(1)
            )
            .await,
            Delivery::Failed { .. }
        ));
        assert_eq!(fs::read_to_string(socket).unwrap(), "unchanged");
    }
}
