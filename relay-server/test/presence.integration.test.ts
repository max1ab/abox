import { execFile, spawn, type ChildProcess } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import Fastify from "fastify";
import type { Pool } from "pg";
import { expect, it } from "vitest";
import { events } from "../src/events.js";

const binary = process.env.AGENT_MAIL_TEST_BIN;
const exec = promisify(execFile);

it.skipIf(!binary)("checks remote bindings on demand across a daemon restart", async () => {
  const root = await mkdtemp(join(tmpdir(), "agent-mail-presence-"));
  const app = Fastify();
  const silent = new AbortController();
  let daemon: ChildProcess | undefined;
  const stop = async () => {
    if (daemon && daemon.exitCode === null && daemon.signalCode === null) {
      const exited = once(daemon, "exit", { signal: AbortSignal.timeout(5000) });
      daemon.kill();
      await exited;
    }
    daemon = undefined;
  };
  const server = "019a4000-0000-7000-8000-000000000001";
  const sender = "019a4000-0000-7000-8000-000000000002";
  const first = "019a4000-0000-7000-8000-000000000003";
  const second = "019a4000-0000-7000-8000-000000000004";
  const cli = resolve(binary!);
  const a = join(root, "a");
  const b = join(root, "b");
  const env = (machine: string) => {
    const variables = { ...process.env, CODEX_HOME: join(machine, "codex"), NO_PROXY: "*" };
    for (const key of ["CODEX_THREAD_ID", "CODEX_SESSION_ID", "CLAUDE_CODE_SESSION_ID", "AGMSG_SYNC_CONNECTION_DIR", "AGMSG_STORAGE_PATH"]) {
      delete (variables as NodeJS.ProcessEnv)[key];
    }
    return variables;
  };
  const run = (machine: string, args: string[], session?: string) => exec(
    cli, ["--data-dir", machine, ...args],
    { cwd: machine, env: { ...env(machine), CODEX_THREAD_ID: session }, timeout: 5000 },
  );
  const bind = async (machine: string, name: string, session: string) => {
    await run(machine, ["join", "dev", "--create", "--name", name, "--type", "codex"]);
    await run(machine, ["bind", "dev", "--name", name, "--type", "codex", "--session-id", session]);
  };
  try {
    await mkdir(a);
    await mkdir(b);
    await bind(a, "sender", sender);
    await bind(b, "agent1", first);
    await bind(b, "agent2", second);
    const configA = JSON.parse(await readFile(join(a, "teams/dev/config.json"), "utf8"));
    const configB = JSON.parse(await readFile(join(b, "teams/dev/config.json"), "utf8"));
    const identity = { protocol_version: 1, server_instance_id: server, team_id: configA.team_id };
    // Presence uses PostgreSQL only for the team identity and the existing
    // LISTEN connection. Stub those while exercising the real HTTP and SSE paths.
    const listener = Object.assign(new EventEmitter(), {
      query: async () => ({ rows: [{ name: "test", application: "test" }] }),
      release: () => {},
    });
    const pool = {
      connect: async () => listener,
      query: async () => ({ rows: [identity] }),
    } as unknown as Pool;
    await events(app, pool);
    app.get("/v1/capabilities", async (_request, reply) => reply.header("agmsg-protocol-version", "1").send({
      ...identity, cipher_profile: "none", accepted_envelope_versions: [1], write_allowed_ciphers: ["none"],
    }));
    const address = await app.listen({ host: "127.0.0.1", port: 0 });
    const binding = {
      endpoint: address, server_instance_id: server, remote_team_id: configA.team_id,
      protocol_version: 1, cipher_profile: "none", disconnected_at: null,
    };
    for (const name of ["agent1", "agent2"]) {
      configA.agents[name] = { member_id: configB.agents[name].member_id, registrations: [] };
    }
    configA.remote_binding = configB.remote_binding = binding;
    await writeFile(join(a, "teams/dev/config.json"), JSON.stringify(configA));
    await writeFile(join(b, "teams/dev/config.json"), JSON.stringify(configB));
    await writeFile(join(a, "body"), "presence test");

    const list = async (...args: string[]) => (await run(a, ["list", ...args])).stdout.trim()
      .split("\n").map(line => JSON.parse(line).name as string);
    const send = () => run(a, ["send", "agent1", "--file", "body"], sender);
    const history = async () => (await run(a, ["history", "--team", "dev"])).stdout;
    const start = async () => {
      daemon = spawn(cli, ["--data-dir", b, "service", "run"], { cwd: b, env: env(b), stdio: "ignore" });
      await expect.poll(() => list(), { timeout: 8000 }).toEqual(["agent1", "agent2", "sender"]);
    };

    expect(await list()).toEqual(["sender"]);
    expect(await list("--all")).toEqual(["agent1", "agent2", "sender"]);
    await expect(send()).rejects.toThrow("recipient is currently unreachable");
    expect(await history()).toBe("");
    await start();
    expect((await send()).stdout).toContain("Queued for remote delivery");
    const queued = await history();
    expect(queued).toContain("presence test");
    await stop();
    const stream = await fetch(`${address}/v1/agent-mail/events`, {
      headers: { "agmsg-protocol-version": "1", "agmsg-team-id": identity.team_id, "agent-mail-presence": "1" },
      signal: silent.signal,
    });
    expect(stream.ok).toBe(true);
    // An open SSE stream alone cannot make either member appear reachable.
    expect(await list()).toEqual(["sender"]);
    await expect(send()).rejects.toThrow("recipient is currently unreachable");
    expect(await history()).toBe(queued);
    silent.abort();
    await start();
    expect((await send()).stdout).toContain("Queued for remote delivery");
  } finally {
    silent.abort();
    try { await stop(); } finally {
      await app.close();
      await rm(root, { recursive: true, force: true });
    }
  }
}, 25000);
