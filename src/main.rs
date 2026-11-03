mod agmsg;
mod board;
mod cli;
mod client;
mod directory;
mod doctor;
mod legacy_export;
mod native_send;
mod server;
mod skill;
mod store;
mod sync;

use std::process::ExitCode;

use clap::Parser;
use cli::{AgmsgCommand, BoardCommand, Cli, Command, DirectoryCommand, SkillCommand, SyncCommand};

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
        Command::Sync {
            command: SyncCommand::Once { team },
        } => {
            let agmsg = agmsg::Agmsg::new(cli.agmsg_dir)?;
            let outcome = sync::once(agmsg.root(), &team).await?;
            client::print_jsonl([outcome])?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor => doctor::run(cli.agmsg_dir, &cli.server).await,
        Command::ExportLegacy { db, output } => {
            legacy_export::export(&db, &output)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Skill {
            command: SkillCommand::Export { directory },
        } => {
            let path = skill::export(&directory)?;
            eprintln!(
                "Exported {}; reload skills in your agent to use it.",
                path.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Serve { db, bind } => {
            server::serve(&db, bind).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Board { command } => {
            let client = client::ApiClient::new(&cli.server)?;
            match command {
                BoardCommand::Post {
                    team,
                    author,
                    body_file,
                    ..
                } => {
                    let body = client::read_body(body_file.as_deref())?;
                    let post = client.publish(team, author, body).await?;
                    client::print_jsonl([post])?;
                }
                BoardCommand::List {
                    team,
                    limit,
                    before,
                } => {
                    client::print_jsonl(client.list(&team, limit, before).await?)?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Directory { command } => {
            let client = client::ApiClient::new(&cli.server)?;
            match command {
                DirectoryCommand::Set {
                    team,
                    agent,
                    display_name,
                    description,
                } => {
                    let profile = client
                        .set_profile(&directory::ProfileInput {
                            team,
                            agent,
                            display_name,
                            description,
                        })
                        .await?;
                    client::print_jsonl([profile])?;
                }
                DirectoryCommand::Get { team, agent } => {
                    client::print_jsonl([client.get_profile(&team, &agent).await?])?;
                }
                DirectoryCommand::List { team } => {
                    let members = agmsg::Agmsg::new(cli.agmsg_dir)?.members(&team)?;
                    let mut entries = Vec::with_capacity(members.len());
                    for member in members {
                        let profile = client.find_profile(&team, &member.name).await?;
                        entries.push(directory::Entry {
                            team: team.clone(),
                            agent: member.name,
                            types: member.types,
                            project: member.project,
                            display_name: profile.as_ref().map(|p| p.display_name.clone()),
                            description: profile.map(|p| p.description),
                        });
                    }
                    // Emit only after every lookup succeeds: service errors must not look
                    // like a complete list with missing metadata.
                    client::print_jsonl(entries)?;
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
                } => {
                    client.send(&team, &from, &to, body_file.as_deref())?;
                    return Ok(ExitCode::SUCCESS);
                }
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
