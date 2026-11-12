mod agmsg;
mod board;
mod cli;
mod client;
mod connect;
mod daemon;
mod directory;
mod doctor;
mod hooks;
mod identity;
mod import;
mod mailbox;
mod native_send;
mod notify;
mod presence;
mod registry;
mod remote;
mod session;
mod setup;
mod skill;
mod store;
mod sync;

use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
use cli::{
    AgmsgCommand, BoardCommand, Cli, Command, DirectoryCommand, SkillCommand, SyncCommand,
    TeamCommand,
};

fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let server_source = matches.value_source("server");
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    // Environment restoration and stdio redirection happen before any Tokio
    // threads exist; the OS supervisor owns this single process throughout.
    let _log = if matches!(
        cli.command,
        Command::Daemon {
            command: cli::DaemonCommand::Launch
        }
    ) {
        match agmsg::Agmsg::new(cli.agmsg_dir.clone())
            .and_then(|local| daemon::prepare_launch(local.root()))
        {
            Ok(log) => Some(log),
            Err(e) => {
                eprintln!("agent-mail: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("agent-mail: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli, server_source)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("agent-mail: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(mut cli: Cli, server_source: Option<ValueSource>) -> Result<ExitCode, String> {
    let server_configured = matches!(
        server_source,
        Some(ValueSource::CommandLine | ValueSource::EnvVariable)
    );
    if let Command::Debug { command } = cli.command {
        cli.command = match command {
            cli::DebugCommand::Hook { command } => Command::Hook { command },
            cli::DebugCommand::Notify { command } => Command::Notify { command },
            cli::DebugCommand::Sync { command } => Command::Sync { command },
            cli::DebugCommand::Doctor => Command::Doctor,
            cli::DebugCommand::Skill { command } => Command::Skill { command },
        };
    }
    match cli.command {
        Command::Debug { .. } => unreachable!(),
        Command::Setup {
            agent_type,
            config,
            skill_dir,
            replace_agmsg,
            default_team,
            machine,
        } => {
            let defaults = setup::Defaults::validated(
                default_team,
                machine,
                (server_source == Some(ValueSource::CommandLine)).then_some(cli.server.as_str()),
            )?;
            let kind = agent_type.map(Ok).unwrap_or_else(identity::infer_type)?;
            let local = agmsg::Agmsg::for_setup(cli.agmsg_dir)?;
            let directory = skill_dir.unwrap_or_else(|| {
                std::path::PathBuf::from(match kind {
                    cli::AgentType::Codex => ".agents/skills/agent-mail",
                    cli::AgentType::ClaudeCode => ".claude/skills/agent-mail",
                })
            });
            // Refuse a different skill before changing agent hook configuration.
            skill::check_install(&directory)?;
            hooks::install(local.root(), kind, config, replace_agmsg, false)?;
            let path = skill::install(&directory)?;
            defaults.save(local.root())?;
            eprintln!(
                "Skill installed at {}. Reload skills and review new hooks in your agent.",
                path.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Start {
            team,
            name,
            machine,
            session,
            project,
            create,
            replace,
            no_login,
        } => {
            // Validate runtime evidence before creating an identity or a team.
            let (kind, id) = identity::runtime(session.agent_type, session.session_id)?;
            let local = agmsg::Agmsg::for_setup(cli.agmsg_dir)?;
            let defaults = setup::Defaults::load(local.root())?;
            let team = team
                .or(defaults.team)
                .ok_or("provide --team or configure setup --default-team")?;
            daemon::check_legacy(local.root())?;
            let bindings = identity::bindings(local.root(), kind, &id)?;
            let existing = bindings.iter().find(|binding| binding.team == team);
            let use_saved_machine =
                !name.contains('@') && existing.is_none_or(|binding| binding.name != name);
            let machine = machine.or_else(|| defaults.machine.filter(|_| use_saved_machine));
            let name = setup::registration_name(name, machine.as_deref())?;
            if existing.is_some_and(|binding| binding.name != name) {
                return Err("this session already has another identity in this team; use that identity or a different receiving session".into());
            }
            let server = if server_configured {
                Some(cli.server.as_str())
            } else {
                defaults.server.as_deref()
            };
            if let Some(server) = server
                && (!create || setup::attachment_pending(local.root(), &team)?)
            {
                setup::import_for_start(
                    &local,
                    &team,
                    server,
                    server_source == Some(ValueSource::CommandLine),
                )
                .await?;
            }
            setup::join(&local, &team, &name, kind, &project, create)?;
            if create && let Some(server) = server {
                connect::publish(local.root(), &team, server).await?;
            }
            setup::bind(&local, &team, &name, kind, &project, Some(id), replace)?;
            client::print_jsonl([daemon::start(local.root(), no_login).await?])?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Leave { team, session } => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            let (kind, id) = identity::runtime(session.agent_type, session.session_id)?;
            let member = identity::resolve(local.root(), team.as_deref(), kind, &id)?;
            registry::leave(local.root(), &member.team, &member.name, kind, &id)?;
            println!("Left {}/{}.", member.team, member.name);
            Ok(ExitCode::SUCCESS)
        }
        Command::Send {
            to,
            team,
            session,
            body_file,
            ..
        } => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            let (kind, id) = identity::runtime(session.agent_type, session.session_id)?;
            let sender = identity::resolve(local.root(), team.as_deref(), kind, &id)?;
            local
                .send(&sender.team, &sender.name, &to, body_file.as_deref())
                .await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::List { team, all } => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            presence::list(local.root(), team, all).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::History {
            team,
            session,
            agent,
            limit,
        } => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            let team = identity::team(local.root(), team, session.agent_type, session.session_id)?;
            client::print_jsonl(mailbox::history(
                local.root(),
                &team,
                agent.as_deref(),
                limit,
            )?)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Status => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            let current = match identity::runtime(None, None) {
                Ok((kind, id)) => match identity::bindings(local.root(), kind, &id) {
                    Ok(bindings) => {
                        serde_json::json!({"type":kind.as_str(),"session_id":id,"identities":bindings})
                    }
                    Err(e) => serde_json::json!({"error":e}),
                },
                Err(e) => serde_json::json!({"error":e}),
            };
            // Status is observational: do not create a notification DB for an unused installation.
            let notifications = if local.root().join("run/notifications.db").is_file() {
                serde_json::to_value(notify::State::open(local.root())?.list()?)
                    .map_err(|e| e.to_string())?
            } else {
                serde_json::json!([])
            };
            client::print_jsonl([serde_json::json!({
                "service":daemon::status(local.root())?,
                "current_session":current,
                "notifications":notifications,
            })])?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Hook { command } => {
            let local = agmsg::Agmsg::for_setup(cli.agmsg_dir)?;
            match command {
                cli::HookCommand::Install {
                    agent_type,
                    config,
                    replace_agmsg,
                } => hooks::install(local.root(), agent_type, config, replace_agmsg, false)?,
                cli::HookCommand::Uninstall { agent_type, config } => {
                    hooks::install(local.root(), agent_type, config, false, true)?
                }
                cli::HookCommand::Run { agent_type } => hooks::run(local.root(), agent_type)?,
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Daemon { command } => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            match command {
                cli::DaemonCommand::Start { no_login } => {
                    client::print_jsonl([daemon::start(local.root(), no_login).await?])?
                }
                cli::DaemonCommand::Stop => daemon::stop(local.root()).await?,
                cli::DaemonCommand::Status => client::print_jsonl([daemon::status(local.root())?])?,
                cli::DaemonCommand::Run | cli::DaemonCommand::Launch => {
                    daemon::run(local.root()).await?
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Inbox {
            agent_type,
            session_id,
            notification,
        } => {
            let (kind, id) = identity::runtime(agent_type, session_id)?;
            notify::inbox(
                agmsg::Agmsg::new(cli.agmsg_dir)?.root(),
                kind,
                Some(id),
                notification,
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Notify { command } => {
            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
            match command {
                cli::NotifyCommand::Once => {
                    client::print_jsonl([notify::once(local.root()).await?])?
                }
                cli::NotifyCommand::Status => {
                    client::print_jsonl(notify::State::open(local.root())?.list()?)?
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Command::Team {
            command: TeamCommand::Connect { team },
        } => {
            connect::run(agmsg::Agmsg::new(cli.agmsg_dir)?.root(), &team, &cli.server).await?;
            Ok(ExitCode::SUCCESS)
        }
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
                    sync::select(agmsg.root(), &team, true)?;
                    client::print_jsonl([daemon::start(agmsg.root(), no_login).await?])?;
                }
                SyncCommand::Disable { team } | SyncCommand::Stop { team } => {
                    sync::select(agmsg.root(), &team, false)?;
                    sync::service::remove(agmsg.root(), &team)?;
                    sync::runtime::stop(agmsg.root(), &team).await?;
                }
                SyncCommand::Start { team } => {
                    sync::select(agmsg.root(), &team, true)?;
                    client::print_jsonl([daemon::start(agmsg.root(), false).await?])?;
                }
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
            let server =
                setup::relay_address(cli.agmsg_dir.clone(), cli.server, server_configured)?;
            let client = client::ApiClient::new(&server)?;
            match command {
                BoardCommand::Post {
                    team,
                    author,
                    selected_team,
                    session,
                    body_file,
                    ..
                } => {
                    let explicit = team.or(selected_team);
                    let (team, author) = if let (Some(team), Some(author)) = (&explicit, &author) {
                        (team.clone(), author.clone())
                    } else {
                        let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
                        let (kind, id) = identity::runtime(session.agent_type, session.session_id)?;
                        let sender =
                            identity::resolve(local.root(), explicit.as_deref(), kind, &id)?;
                        (sender.team, author.unwrap_or(sender.name))
                    };
                    let body = client::read_body(body_file.as_deref())?;
                    let post = client.publish(team, author, body).await?;
                    client::print_jsonl([post])?;
                }
                BoardCommand::List {
                    team,
                    selected_team,
                    session,
                    limit,
                    before,
                } => {
                    let team = match team.or(selected_team) {
                        Some(team) => team,
                        None => {
                            let local = agmsg::Agmsg::new(cli.agmsg_dir)?;
                            identity::team(
                                local.root(),
                                None,
                                session.agent_type,
                                session.session_id,
                            )?
                        }
                    };
                    client::print_jsonl(client.list(&team, limit, before).await?)?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Directory { command } => {
            let server =
                setup::relay_address(cli.agmsg_dir.clone(), cli.server, server_configured)?;
            let client = client::ApiClient::new(&server)?;
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
                } => client.send(&team, &from, &to, body_file.as_deref()).await?,
                AgmsgCommand::Inbox { team, agent } => {
                    mailbox::inbox(client.root(), &team, &agent)?
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
