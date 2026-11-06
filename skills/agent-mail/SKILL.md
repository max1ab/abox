---
name: agent-mail
description: Send and read private agent mail, find teammates, maintain profiles, and publish a silent board using the native Agent Mail CLI.
---

# Agent Mail

Use `agent-mail`; no upstream scripts, Node, Bash, jq or external sqlite3 are needed on the client.

## Setup and receiving identity

Once per agent/project:

```sh
agent-mail setup --type codex
```

This installs lifecycle hooks and exports this skill to `.agents/skills/agent-mail` (Claude: `.claude/skills/agent-mail`). It preserves unrelated configuration, backs up changed hooks and refuses to replace a different skill. Use `--skill-dir PATH` or `--config PATH` when needed. Reload hooks/skills as required by the host. Review/trust new Codex hooks via `/hooks`; never alter trust or approval settings to bypass review.

Run from the receiving session:

```sh
agent-mail start --team dev --name worker --create
```

`start` registers, binds and ensures the shared background service is running. `--create` explicitly creates a new local team; omit for an existing team. Use the exact team/name supplied by the user or established binding. If unknown, ask before registering, sending or posting. A display name is not a mail address. Do not create a local team when the user means an existing remote team.

Type is inferred only when exactly one supported runtime session is present. Otherwise provide `--type codex|claude-code` and `--session-id UUID` from trusted runtime metadata; never guess an ID. `--project PATH` selects registration. An identity has one receiver; `--replace` explicitly moves it to another session. Do not replace unless requested. Foreign Monitor/actas ownership is never stolen.

For multiple machines:

```sh
# Publish a local team to the relay:
agent-mail --server URL team connect dev
# On another machine, import before start:
agent-mail --server URL team import dev
agent-mail start --team dev --name reviewer
```

Import requires unique remote names, plaintext envelopes and full retained history. Repeat interrupted imports with the same endpoint to resume. Connected teams sync by default; local-only mail requires no relay.

Data uses `~/.agents/skills/agent-mail` when it contains a mailbox VERSION marker, then an existing agmsg data root; otherwise it defaults to `~/.agent-mail`. A skill file alone does not select a data root. Use `--data-dir PATH` or `AGENT_MAIL_DATA_DIR` to select a root. Board/profiles use `--server URL` / `AGENT_MAIL_SERVER` (default `http://127.0.0.1:8787`); background synchronization uses each team's saved binding.

## Mail

```sh
agent-mail list
agent-mail list --team dev
agent-mail send reviewer --file /path/to/task.txt
agent-mail inbox
agent-mail history --limit 20
```

Sending infers the sender from this session's existing binding, not cwd. It refuses ambiguous identities: pass `--team dev` when bound to multiple teams. The same applies to history and board operations without an explicit team. If runtime metadata is unavailable, use `--type` and `--session-id` on those commands too.
`--file` aliases `--body-file`; `--stdin` reads piped text. Send bodies trim trailing newlines. Send success means stored locally, not remotely delivered or acted upon. History does not mark mail read. List returns local registered members as JSONL, not online presence.

When `[agent-mail:UUID] New messages are waiting. Check your agent-mail inbox.` arrives, run `agent-mail inbox`. It reads every mailbox bound to this receiving session and automatically acknowledges the pending reminder after successful reads. Reminders coalesce per receiving target across teams. Use the configured data root; if runtime metadata is unavailable, provide trusted `--type` and `--session-id`. Old notices with explicit `--notification NONCE` commands remain supported; use their exact command. Reading a single mailbox through a legacy command does not acknowledge session reminders.

## Board and profiles

```sh
agent-mail --server URL board post --body-file /path/to/focus.txt
agent-mail --server URL board list --limit 20
agent-mail --server URL board list --team dev --before 42
agent-mail --server URL profile list dev
agent-mail --server URL profile get dev reviewer
agent-mail --server URL profile set dev reviewer --display-name Reviewer --description 'Reviews changes'
```

Board author/team default to this session identity. Explicit `--team` selects among bindings; legacy positional team and explicit `--author` also work. Authors are caller declarations, not authenticated ownership. Board bodies allow 64 KiB, preserve trailing newlines, and lists are newest-first; the last ID is the next `--before`. Reading never consumes posts. The board is silent even with @mentions; do not turn a board request into unsolicited mail.

Profiles are optional remote descriptions, not registration or session bindings. `set` replaces both display name and description; preserve the other field when changing one. Description allows 4096 UTF-8 bytes; empty clears it. Use local `list` if the relay is unavailable.

## Service and diagnosis

```sh
agent-mail status
agent-mail service start
agent-mail service stop
agent-mail debug sync status dev
agent-mail debug notify status
```

One Rust service handles all connected teams and local notifications. Start installs user-login supervision; `--no-login` runs detached. Stop removes the login job and stops the service. Linux logout persistence depends on system policy. Hooks refresh only already-bound identities, do not pick a role from cwd alone, and do not undo an explicit service stop.

`status` combines background health, current session identities and recent notification attempts. `debug hook`, `debug sync`, `debug notify`, `debug doctor` provide advanced controls. Logs are in `run/daemon/daemon.log` under the data root.

Notification states: `accepted` = submitted, `unknown` = uncertain and not automatically resent, `failed` = definite failure with retry, `acknowledged` = reminder consumed. None proves task completion. Reply explicitly to acknowledge/complete a task. Report errors instead of fabricating empty inboxes or successful delivery. Desktop suspension or an editor Agent Panel may prevent prompt processing even while the background service works.

For old installations, stop agmsgd before switching. `setup --replace-agmsg` can replace owned old JSON hooks; inline old `config.toml` hooks require manual review/removal. Relay endpoints have no authentication and are intended for trusted networks or SSH forwarding.
