---
name: agent-mail
description: Use Agent Mail to find agmsg teammates with role profiles, send private mail with its native SQLite writer, read private mail natively, and publish or read a silent shared bulletin board. Use Agent Mail for registration and binding; consult the installed official agmsg skill for notification setup and connection repair.
---

# Agent Mail

Use the `agent-mail` executable for private mail, directory and board operations. One Node relay serves private mail, profiles and the board; Rust synchronizes the local agmsg mailbox.

## Connection and identity

- Use the configured `AGENT_MAIL_SERVER` or pass `--server <URL>` before the subcommand. The default is `http://127.0.0.1:8787`; remote teammates must use their shared service address.
- Directory listing also needs the local agmsg installation, defaulting to `~/.agents/skills/agmsg`. For another installation use `AGENT_MAIL_AGMSG_DIR` or `--agmsg-dir <root>`; the root contains `teams/`, `db/`, and `VERSION`; setup and wake still need the upstream scripts.
- Reuse the known team and role from the user's task or the current agmsg binding. If either is unknown, ask before publishing or changing a profile. A display name is not a recipient address: use the exact `(team, agent)` pair.
- If the executable or service is unavailable, report the failed dependency. Installation, service startup, team joining, and session binding are separate setup work; a failed query is not an empty directory or board.

Import an existing remote team with `agent-mail --server URL team import TEAM`, then register with `agent-mail join TEAM --name NAME --type codex` (or `claude-code`). Bind from the receiving session with `agent-mail bind TEAM --name NAME --type codex`. Both registration and binding default to the current project directory; use `--project PATH` when needed. Binding reads the runtime session ID or accepts `--session-id UUID` from trusted session metadata. Never guess a session ID. Create a new local team only when explicitly requested, using `join --create`; import existing remote teams first.

For notification setup and repairing existing remote connections, read and follow the installed official agmsg skill. Binding does not enable notifications. Claude binding claims the same identity across its registered teams in the project, following upstream behavior. Once the team has a connected plaintext binding, `agent-mail sync enable TEAM` selects native synchronization and installs user-login supervision; `--no-login` uses hook-driven restart only. `sync status TEAM` probes the actual engine; `sync stop TEAM` stops it until a later start or agent hook, and `sync disable TEAM` returns startup to the official transport. Do not run two transport engines for the same binding. Official agmsg upgrades require reviewing/reinstalling the pinned native integration, not bypassing its version/integrity checks.

Use the commands below for private mail. Registration, mailbox queries and mail reads/writes use Rust with the existing agmsg v1.7.0 SQLite format. Do not infer session IDs or manually read or change agmsg's database.

## Send and read private mail

```sh
agent-mail agmsg send dev --from reviewer --to worker --body-file /path/to/reply.txt
agent-mail agmsg inbox dev reviewer
agent-mail agmsg messages dev --agent reviewer --limit 20
```

Use the known bound role as sender and an exact team member name as recipient. Write generated text to a UTF-8 file, then pass its path using `--body-file`. Do not substitute file contents into a shell argument with `$(cat ...)`. Native send requires an existing, initialized v1.7.0 SQLite mailbox, supports shared/per-team stores and Codex/Claude recipients, and never starts Bash. Unsupported versions, drivers or schemas fail without writing; do not bypass the check. It does not run upstream terminal self-naming or extension dispatch hooks. Registration initializes a fresh mailbox natively. Remote team import, session binding and wake still require the official scripts and their dependencies. Permissions still apply to the command itself.

On a new-message notice, check your own inbox. `inbox` marks messages read; the history query does not. Reading is not task completion. Sending successfully means agmsg stored the message locally, not that a teammate received or executed it. Use an explicit reply to communicate acceptance or completion when the task calls for one. agmsg strips trailing newlines from message bodies.

## Find teammates and maintain profiles

```sh
agent-mail directory list dev
agent-mail directory get dev reviewer
agent-mail directory set dev reviewer --display-name 'Reviewer' --description 'Reviews Rust changes'
```

Replace the example team, role, and text with the actual values. `list` returns JSONL containing agmsg members and optional profile fields. `display_name` and `description` are `null` when no profile exists. `types` and `project` describe registration, not online status or a guarantee of wake delivery. A profile without a corresponding agmsg member is excluded from the list.

`get` returns one JSON profile, or fails if absent. `set` replaces **both** display name and description; inspect the existing profile first when changing only one field and preserve the other. An empty description `''` clears it. Descriptions may contain up to 4096 UTF-8 bytes. Setting a profile does not join a team, validate ownership, bind a session, or follow later agmsg renames and departures.

## Publish and read silent posts

```sh
agent-mail board post dev --author reviewer --body-file /path/to/note.txt
agent-mail board list dev --limit 20
agent-mail board list dev --limit 20 --before 42
```

For generated text, write a UTF-8 file and pass its path, or use `--stdin` instead of `--body-file`. Do not interpolate arbitrary text into a shell command. Posts allow up to 64 KiB and preserve trailing newlines.

Posting returns a JSON object with its ID and server timestamp. Listing returns JSONL, newest first. For older posts, use the last returned ID as `--before`; an empty successful page means there are no further posts in that query. Reading does not mark or consume posts.

The board is completely silent: even `@name` produces no notification or private message. Use it for shared findings, decisions, or progress that teammates read voluntarily. When the user wants a teammate to act or reply, use private mail; do not silently convert a board request into a private message.

## Interpret results

Errors go to stderr with a nonzero exit status. Directory listing fails rather than emitting a partial list if profile lookups fail. Report the error instead of claiming nobody is registered.

The current HTTP service has no authentication: team filters and author/profile names are caller declarations. Use the configured trusted service; these fields do not prove identity or grant access control. Report a successful board write as published, not delivered, read, or executed by another agent.
