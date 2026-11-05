mod agmsg;
mod board;
mod cli;
mod client;
mod directory;
mod doctor;
mod import;
mod mailbox;
mod native_send;
mod registry;
mod remote;
mod session;
mod setup;
mod skill;
mod store;
mod sync;

use std::process::ExitCode;

use clap::Parser;
use cli::{
    AgmsgCommand, BoardCommand, Cli, Command, DirectoryCommand, SkillCommand, SyncCommand,
    TeamCommand,
};

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
        Command::Team {
            command: TeamCommand::Import { team },
        } => {
            setup::import(&agmsg::Agmsg::for_setup(cli.agmsg_dir)?, &team, &cli.server).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Join {
            team,
            name,
            agent_type,
            project,
            create,
        } => {
            let local = if create {
                agmsg::Agmsg::for_setup(cli.agmsg_dir)?
            } else {
                agmsg::Agmsg::new(cli.agmsg_dir)?
            };
            setup::join(&local, &team, &name, agent_type, &project, create)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Bind {
            team,
            name,
            agent_type,
            project,
            session_id,
            replace,
        } => {
            setup::bind(
                &agmsg::Agmsg::new(cli.agmsg_dir)?,
                &team,
                &name,
                agent_type,
                &project,
                session_id,
                replace,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Sync { command } => {
            let agmsg = agmsg::Agmsg::new(cli.agmsg_dir)?;
            match command {
                SyncCommand::Enable { team, no_login } => {
                    sync::bridge::enable(&agmsg, &team, no_login).await?;
                    client::print_jsonl([sync::runtime::status(agmsg.root(), &team)?])?;
                }
                SyncCommand::Disable { team } => {
                    sync::bridge::disable(agmsg.root(), &team).await?;
                }
                SyncCommand::Start { team } => {
                    client::print_jsonl([sync::runtime::start(agmsg.root(), &team)?])?;
                }
                SyncCommand::Stop { team } => {
                    let _control = sync::runtime::ControlLock::acquire(agmsg.root(), &team)?;
                    sync::service::pause(agmsg.root(), &team)?;
                    sync::runtime::stop(agmsg.root(), &team).await?;
                }
                SyncCommand::Launch { team } => sync::runtime::launch(agmsg.root(), &team)?,
                SyncCommand::Once { team } => {
                    client::print_jsonl([sync::once(agmsg.root(), &team).await?])?;
                }
                SyncCommand::Run { team, interval } => {
                    sync::runtime::run(
                        agmsg.root(),
                        &team,
                        std::time::Duration::from_secs(interval),
                    )
                    .await?;
                }
                SyncCommand::Status { team } => {
                    client::print_jsonl([sync::runtime::status(agmsg.root(), &team)?])?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor => doctor::run(cli.agmsg_dir, &cli.server).await,
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
            match command {
                AgmsgCommand::Version => println!(
                    "agent-mail {} (mailbox format 1)",
                    env!("CARGO_PKG_VERSION")
                ),
                AgmsgCommand::Teams => client::print_jsonl(mailbox::teams(client.root())?)?,
                AgmsgCommand::Members { team } => client::print_jsonl(client.members(&team)?)?,
                AgmsgCommand::Messages { team, agent, limit } => client::print_jsonl(
                    mailbox::history(client.root(), &team, agent.as_deref(), limit)?,
                )?,
                AgmsgCommand::Send {
                    team,
                    from,
                    to,
                    body_file,
                    ..
                } => client.send(&team, &from, &to, body_file.as_deref())?,
                AgmsgCommand::Inbox { team, agent } => {
                    mailbox::inbox(client.root(), &team, &agent)?
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
