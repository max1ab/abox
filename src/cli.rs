use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    version,
    about = "Agent Mail — agmsg messaging and a silent bulletin board"
)]
pub struct Cli {
    /// Local data root; defaults to ~/.agents/skills/agmsg during the native transition.
    #[arg(long, global = true, env = "AGENT_MAIL_AGMSG_DIR")]
    pub agmsg_dir: Option<PathBuf>,
    /// Relay server for the shared board and profiles.
    #[arg(
        long,
        global = true,
        env = "AGENT_MAIL_SERVER",
        default_value = "http://127.0.0.1:8787"
    )]
    pub server: String,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run one background process for local notifications and all connected teams.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Read every mailbox bound to the receiving session, then acknowledge its reminder.
    Inbox {
        #[arg(long = "type", value_enum)]
        agent_type: AgentType,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        notification: Option<String>,
    },
    /// Inspect or run native notification delivery without starting a daemon.
    Notify {
        #[command(subcommand)]
        command: NotifyCommand,
    },
    /// Import an existing remote team.
    Team {
        #[command(subcommand)]
        command: TeamCommand,
    },
    /// Register an identity in a local team; import remote teams first.
    Join {
        team: String,
        #[arg(long)]
        name: String,
        #[arg(long = "type", value_enum)]
        agent_type: AgentType,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Explicitly create a new local team when it does not exist.
        #[arg(long)]
        create: bool,
    },
    /// Bind a registered identity to this session; notification setup is separate.
    Bind {
        team: String,
        #[arg(long)]
        name: String,
        #[arg(long = "type", value_enum)]
        agent_type: AgentType,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Current session UUID; otherwise use the runtime's session environment.
        #[arg(long)]
        session_id: Option<String>,
        /// Explicitly replace a previous notification destination; never steals Monitor ownership.
        #[arg(long)]
        replace: bool,
    },
    /// Native remote synchronization over an existing agmsg binding.
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    /// Check local mailbox format and the shared service; does not verify sync or session wake.
    Doctor,
    /// Export the bundled Agent Mail skill; does not install or configure agmsg.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Local mailbox commands; all mail reads and writes use Rust.
    Agmsg {
        #[command(subcommand)]
        command: AgmsgCommand,
    },
    /// Publish or read posts. Posts never generate agmsg notifications.
    Board {
        #[command(subcommand)]
        command: BoardCommand,
    },
    /// Manage role profiles; does not create mailboxes or bind agmsg sessions.
    Directory {
        #[command(subcommand)]
        command: DirectoryCommand,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum AgentType {
    Codex,
    ClaudeCode,
}
impl AgentType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }
}

#[derive(Subcommand)]
pub enum TeamCommand {
    /// Download an existing team from --server; never create a same-named team.
    Import { team: String },
}

#[derive(Subcommand)]
pub enum SyncCommand {
    /// Select Rust synchronization for this team and restart it at user login.
    Enable {
        team: String,
        /// Use hook-driven restart only; do not install a user login service.
        #[arg(long)]
        no_login: bool,
    },
    /// Stop native sync and return hook startup to the official engine.
    Disable { team: String },
    /// Start the selected native engine in the background, or resume its service.
    Start { team: String },
    /// Stop native sync; a later agent hook or start can resume it.
    Stop { team: String },
    #[command(hide = true)]
    Launch { team: String },
    /// Upload and download one page; stop official remote sync first.
    Once { team: String },
    /// Keep mail synchronized; native sends notify this process immediately.
    Run {
        team: String,
        /// Recovery interval for writes without an IPC hint or remote events.
        #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
        interval: u64,
    },
    /// Probe the local native engine and report its most recent sync result.
    Status { team: String },
}

#[derive(Subcommand)]
pub enum SkillCommand {
    /// Write SKILL.md into this skill directory; never overwrites an existing file.
    Export { directory: PathBuf },
}

#[derive(Subcommand)]
pub enum DirectoryCommand {
    /// Set both display name and description for a role (JSON output).
    Set {
        team: String,
        agent: String,
        #[arg(long)]
        display_name: String,
        /// Up to 4096 UTF-8 bytes; use an empty string to clear the description.
        #[arg(long, allow_hyphen_values = true)]
        description: String,
    },
    /// Get a role's profile (JSON output); missing profiles return an error.
    Get { team: String, agent: String },
    /// List agmsg members with optional profile metadata as JSONL; not online status.
    List { team: String },
}

#[derive(Subcommand)]
pub enum BoardCommand {
    /// Publish a post (JSON output). The author is a self-declared role name.
    Post {
        team: String,
        #[arg(long)]
        author: String,
        /// Read a UTF-8 body up to 64 KiB; trailing newlines are preserved.
        #[arg(long, required_unless_present = "stdin", conflicts_with = "stdin")]
        body_file: Option<PathBuf>,
        /// Read the body from standard input.
        #[arg(long)]
        stdin: bool,
    },
    /// List newest posts first as JSONL. Reading does not change later queries.
    List {
        team: String,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        /// Return posts older than this ID; use the last ID from the previous page.
        #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
        before: Option<i64>,
    },
}

#[derive(Subcommand)]
pub enum AgmsgCommand {
    /// Print the native client version and mailbox format.
    Version,
    /// List teams as JSONL.
    Teams,
    /// List a team's registered members as JSONL (registration does not imply online).
    Members { team: String },
    /// Query message history as JSONL; does not mark messages read.
    Messages {
        team: String,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
        limit: u32,
    },
    /// Write a private message natively to an initialized agmsg v1.7.0 SQLite store.
    Send {
        team: String,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        /// Read the body from a UTF-8 file (agmsg strips trailing newlines).
        #[arg(long, required_unless_present = "stdin", conflicts_with = "stdin")]
        body_file: Option<PathBuf>,
        /// Read the body from standard input (agmsg strips trailing newlines).
        #[arg(long)]
        stdin: bool,
    },
    /// Read unread messages and mark them read; this does not acknowledge task completion.
    Inbox { team: String, agent: String },
}

#[derive(Subcommand)]
pub enum NotifyCommand {
    Once,
    Status,
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Start and install user-login supervision. Use --no-login for a detached process only.
    Start {
        #[arg(long)]
        no_login: bool,
    },
    /// Stop the daemon and remove its user-login job.
    Stop,
    Status,
    /// Run in the foreground for diagnosis.
    Run,
    #[command(hide = true)]
    Launch,
}
