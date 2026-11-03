use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Agent Mail — agmsg messaging and a silent bulletin board"
)]
pub struct Cli {
    /// agmsg installation root (contains scripts/); defaults to ~/.agents/skills/agmsg.
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
    /// Export the stopped legacy board/profile SQLite database for Node import.
    ExportLegacy {
        #[arg(long)]
        db: PathBuf,
        /// A new JSON file; existing files are never overwritten.
        #[arg(long)]
        output: PathBuf,
    },
    /// Check local agmsg and the shared service; does not verify sync or session wake.
    Doctor,
    /// Export the bundled Agent Mail skill; does not install or configure agmsg.
    Skill {
        #[command(subcommand)]
        command: SkillCommand,
    },
    /// Use an existing agmsg installation; native send, official scripts for reads.
    Agmsg {
        #[command(subcommand)]
        command: AgmsgCommand,
    },
    /// Run the Agent Mail service (no authentication; use a trusted network).
    Serve {
        /// A separate Agent Mail database, not an agmsg message database.
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8790")]
        bind: SocketAddr,
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
    /// Print the installed agmsg version.
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
