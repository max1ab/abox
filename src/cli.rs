use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    version,
    about = "Private agent mail, team members and a silent shared board"
)]
pub struct Cli {
    /// Data root; reuses a marked agent-mail/agmsg skill data directory, otherwise ~/.agent-mail.
    #[arg(
        long = "data-dir",
        value_name = "PATH",
        alias = "agmsg-dir",
        global = true,
        env = "AGENT_MAIL_DATA_DIR"
    )]
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
    /// Install lifecycle hooks and export the project skill (once per agent/project).
    Setup {
        #[arg(long = "type", value_enum)]
        agent_type: Option<AgentType>,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        skill_dir: Option<PathBuf>,
        #[arg(long)]
        replace_agmsg: bool,
    },
    /// Register, bind this receiving session and ensure the background service is running.
    Start {
        #[arg(long)]
        team: String,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        session: SessionArgs,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Create a new local team explicitly; import existing remote teams first.
        #[arg(long)]
        create: bool,
        /// Explicitly move this identity from another session.
        #[arg(long)]
        replace: bool,
        /// Run detached without installing a user-login service.
        #[arg(long)]
        no_login: bool,
    },
    /// Send as the identity bound to this session. Ambiguous bindings require --team.
    Send {
        to: String,
        #[arg(long)]
        team: Option<String>,
        #[command(flatten)]
        session: SessionArgs,
        #[arg(
            long,
            alias = "file",
            required_unless_present = "stdin",
            conflicts_with = "stdin"
        )]
        body_file: Option<PathBuf>,
        #[arg(long)]
        stdin: bool,
    },
    /// List local and currently reachable remote members (JSONL).
    List {
        #[arg(long)]
        team: Option<String>,
        /// Show the full registered roster without checking reachability.
        #[arg(long)]
        all: bool,
    },
    /// Query mail history without marking it read (JSONL).
    History {
        #[arg(long)]
        team: Option<String>,
        #[command(flatten)]
        session: SessionArgs,
        #[arg(long)]
        agent: Option<String>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
        limit: u32,
    },
    /// Show service health, session bindings and notification state together.
    Status,
    /// Advanced hook, notification and synchronization diagnostics.
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
    /// Install or run native lifecycle hooks; never changes hook trust settings.
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    /// Run one background process for local notifications and all connected teams.
    #[command(name = "service", alias = "daemon")]
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Read every mailbox bound to this session and acknowledge its pending reminder.
    Inbox {
        #[arg(long = "type", value_enum)]
        agent_type: Option<AgentType>,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        notification: Option<String>,
    },
    /// Inspect or run native notification delivery without starting a daemon.
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    #[command(hide = true)]
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
    /// Advanced team synchronization controls.
    #[command(hide = true)]
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    /// Check local mailbox format and the shared service; does not verify sync or session wake.
    #[command(hide = true)]
    Doctor,
    /// Export the bundled Agent Mail skill; does not install or configure agmsg.
    #[command(hide = true)]
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Local mailbox commands; all mail reads and writes use Rust.
    #[command(hide = true)]
    Agmsg {
        #[command(subcommand)]
        command: AgmsgCommand,
    },
    /// Publish or read silent shared posts.
    Board {
        #[command(subcommand)]
        command: BoardCommand,
    },
    /// Manage optional remote member profiles.
    #[command(name = "profile", alias = "directory")]
    Directory {
        #[command(subcommand)]
        command: DirectoryCommand,
    },
}

#[derive(clap::Args)]
pub struct SessionArgs {
    /// Agent type; inferred when exactly one supported runtime session is present.
    #[arg(long = "type", value_enum)]
    pub agent_type: Option<AgentType>,
    /// Session UUID from trusted runtime metadata, when unavailable in the environment.
    #[arg(long)]
    pub session_id: Option<String>,
}

#[derive(Subcommand)]
pub enum DebugCommand {
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    Notify {
        #[command(subcommand)]
        command: NotifyCommand,
    },
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    Doctor,
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
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
    /// Register a locally created plaintext team on --server.
    Connect { team: String },
}

#[derive(Subcommand)]
pub enum SyncCommand {
    /// Select Rust synchronization for this team and restart it at user login.
    Enable {
        team: String,
        /// Start detached without installing a user login service.
        #[arg(long)]
        no_login: bool,
    },
    /// Pause this team inside the shared Rust daemon.
    Disable { team: String },
    /// Start the selected native engine in the background, or resume its service.
    Start { team: String },
    /// Pause this team until sync start or enable.
    Stop { team: String },
    /// Upload and download one page without starting the shared service.
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
        /// Team, otherwise inferred from the current session.
        team: Option<String>,
        #[arg(long = "team", conflicts_with = "team")]
        selected_team: Option<String>,
        #[command(flatten)]
        session: SessionArgs,
        #[arg(long)]
        author: Option<String>,
        /// Read a UTF-8 body up to 64 KiB; trailing newlines are preserved.
        #[arg(long, required_unless_present = "stdin", conflicts_with = "stdin")]
        body_file: Option<PathBuf>,
        /// Read the body from standard input.
        #[arg(long)]
        stdin: bool,
    },
    /// List newest posts first as JSONL. Reading does not change later queries.
    List {
        team: Option<String>,
        #[arg(long = "team", conflicts_with = "team")]
        selected_team: Option<String>,
        #[command(flatten)]
        session: SessionArgs,
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

#[derive(Subcommand)]
pub enum HookCommand {
    Install {
        #[arg(long = "type", value_enum)]
        agent_type: AgentType,
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        replace_agmsg: bool,
    },
    Uninstall {
        #[arg(long = "type", value_enum)]
        agent_type: AgentType,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    #[command(hide = true)]
    Run {
        #[arg(long = "type", value_enum)]
        agent_type: AgentType,
    },
}
