mod agmsg;
mod board;
mod board_client;
mod cli;
mod directory;
mod server;

use std::process::ExitCode;

use clap::Parser;
use cli::{AgmsgCommand, BoardCommand, Cli, Command};

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("agent-mail: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode, String> {
    match cli.command {
        Command::Serve { db, bind } => {
            server::serve(&db, bind).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Board { command } => {
            let client = board_client::BoardClient::new(&cli.server)?;
            match command {
                BoardCommand::Post {
                    team,
                    author,
                    body_file,
                    ..
                } => {
                    let body = board_client::read_body(body_file.as_deref())?;
                    let post = client.publish(team, author, body).await?;
                    board_client::print_jsonl([post])?;
                }
                BoardCommand::List {
                    team,
                    limit,
                    before,
                } => {
                    board_client::print_jsonl(client.list(&team, limit, before).await?)?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Agmsg { command } => {
            let client = agmsg::Agmsg::new(cli.agmsg_dir)?;
            let status = match command {
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
            }?;
            Ok(ExitCode::from(
                status
                    .code()
                    .and_then(|code| u8::try_from(code).ok())
                    .unwrap_or(1),
            ))
        }
    }
}
