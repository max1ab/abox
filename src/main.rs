mod agmsg;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Agent Mail — agmsg integration CLI")]
struct Cli {
    /// agmsg installation root (contains scripts/); defaults to ~/.agents/skills/agmsg.
    #[arg(long, global = true, env = "AGENT_MAIL_AGMSG_DIR")]
    agmsg_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Use an existing agmsg installation through its official Bash scripts.
    Agmsg {
        #[command(subcommand)]
        command: AgmsgCommand,
    },
}

#[derive(Subcommand)]
enum AgmsgCommand {
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
    /// Send a private message. Success means stored by agmsg, not delivered or executed.
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = agmsg::Agmsg::new(cli.agmsg_dir).and_then(|client| match cli.command {
        Command::Agmsg { command } => match command {
            AgmsgCommand::Version => client.run("agmsg", &["--version"]),
            AgmsgCommand::Teams => client.run("api.sh", &["get", "teams"]),
            AgmsgCommand::Members { team } => {
                client.run("api.sh", &["get", "teams", &team, "members"])
            }
            AgmsgCommand::Messages { team, agent, limit } => {
                let limit = limit.to_string();
                let mut args = vec!["get", "teams", &team, "messages", "--limit", &limit];
                if let Some(agent) = agent.as_deref() {
                    args.extend(["--agent", agent]);
                }
                client.run("api.sh", &args)
            }
            AgmsgCommand::Send {
                team,
                from,
                to,
                body_file,
                ..
            } => client.send(&team, &from, &to, body_file.as_deref()),
            AgmsgCommand::Inbox { team, agent } => client.run("inbox.sh", &[&team, &agent]),
        },
    });

    match result {
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1),
        ),
        Err(error) => {
            eprintln!("agent-mail: {error}");
            ExitCode::FAILURE
        }
    }
}
