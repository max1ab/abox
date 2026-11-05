import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { Pool } from "pg";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { migrate } from "@agmsg/reference-server/dist/src/db.js";
import { createRelayApp } from "../src/app.js";
import { migrateCollaboration } from "../src/collaboration.js";

const databaseUrl = process.env.TEST_DATABASE_URL;
(databaseUrl ? describe : describe.skip)("collaboration PostgreSQL and CLI compatibility", () => {
  const schema = `agent_mail_test_${randomBytes(8).toString("hex")}`;
  let admin: Pool;
  let pool: Pool;
  let app: ReturnType<typeof createRelayApp>;
  beforeAll(async () => {
    admin = new Pool({ connectionString: databaseUrl });
    await admin.query(`CREATE SCHEMA ${schema}`);
    pool = new Pool({ connectionString: databaseUrl, options: `-c search_path=${schema}` });
    await migrate(pool);
    await migrateCollaboration(pool);
    await migrateCollaboration(pool);
    app = createRelayApp(pool, { databaseUrl: databaseUrl!, host: "127.0.0.1", port: 8787, logLevel: "silent", retentionMaxLiveMessages: null });
    await app.ready();
  });
  afterAll(async () => {
    await app?.close();
    await pool?.end();
    if (admin) { await admin.query(`DROP SCHEMA ${schema} CASCADE`); await admin.end(); }
  });
  const publish = (team: string, body: string, author = "mac") => app.inject({ method: "POST", url: "/v1/board/posts", payload: { team, author, body } });
  it("paginates by team, persists mentions and keeps board writes out of mail", async () => {
    const a = await publish("pages", "one\n@dell");
    const b = await publish("pages", "two 'quoted'");
    await publish("other", "hidden");
    expect(a.statusCode).toBe(201);
    const page = await app.inject("/v1/board/posts?team=pages&limit=1");
    expect(page.json()).toEqual([b.json()]);
    const older = await app.inject(`/v1/board/posts?team=pages&before=${b.json().id}`);
    expect(older.json()).toEqual([a.json()]);
    expect((await app.inject("/v1/board/posts?team=pages")).json()).toHaveLength(2);
    expect((await pool.query("SELECT count(*) FROM messages")).rows[0].count).toBe("0");
    expect(page.headers).not.toHaveProperty("agmsg-protocol-version");
    expect((await app.inject("/v1/agent-mail/health")).json()).toEqual({ service: "agent-mail", status: "ok" });
  });
  it("checks GET labels and exact UTF-8, JSON-escaping and character boundaries", async () => {
    expect((await publish("x".repeat(128), "x".repeat(65536))).statusCode).toBe(201);
    expect((await publish("中文", "中".repeat(21845) + "x")).statusCode).toBe(201);
    expect((await publish("escaped", "\u0001".repeat(65536))).statusCode).toBe(201);
    for (const [team, body] of [["x".repeat(129), "ok"], ["valid", "x".repeat(65537)], ["valid", "中".repeat(21846)], ["valid", "\0"], ["valid", " \n"]]) {
      expect((await publish(team!, body!)).statusCode).toBe(400);
    }
    for (const query of ["team=%0A", "team=x&limit=0", "team=x&before=-1", "team=x&before=9007199254740993", "team=x&limit=101", "team=x&limit=1&limit=2"]) {
      expect((await app.inject(`/v1/board/posts?${query}`)).statusCode).toBe(400);
    }
    for (const query of ["team=%0A&agent=a", "team=x&agent=%00", "team=x"]) {
      expect((await app.inject(`/v1/directory/profiles?${query}`)).statusCode).toBe(400);
    }
  });
  it.skipIf(!process.env.AGENT_MAIL_TEST_BIN)("serves the Rust CLI over a real HTTP socket", async () => {
    const address = await app.listen({ host: "127.0.0.1", port: 0 });
    const cli = resolve(process.env.AGENT_MAIL_TEST_BIN!);
    const run = async (args: string[]) => {
      const { stdout } = await promisify(execFile)(cli, ["--server", address, ...args], { timeout: 10000 });
      return JSON.parse(stdout);
    };
    expect((await run(["directory", "set", "cli", "mac", "--display-name", "Mac", "--description", "真实客户端"])).description).toBe("真实客户端");
    expect((await run(["directory", "get", "cli", "mac"])).display_name).toBe("Mac");
    expect(await run(["board", "list", "pages", "--limit", "1"])).toMatchObject({ team: "pages" });
  });
  it("updates profiles without registering a mailbox; reports a stable missing code", async () => {
    const profile = { team: "profiles", agent: "dell", display_name: "Dell", description: "x".repeat(4096) };
    const first = await app.inject({ method: "PUT", url: "/v1/directory/profiles", payload: profile });
    expect(first.statusCode).toBe(200);
    const changed = await app.inject({ method: "PUT", url: "/v1/directory/profiles", payload: { ...profile, description: "" } });
    expect(changed.json().description).toBe("");
    expect((await app.inject("/v1/directory/profiles?team=profiles&agent=dell")).json()).toEqual(changed.json());
    expect((await app.inject({ method: "PUT", url: "/v1/directory/profiles", payload: { ...profile, description: "x".repeat(4097) } })).statusCode).toBe(400);
    expect((await app.inject("/v1/directory/profiles?team=other&agent=dell")).json()).toMatchObject({ code: "profile_not_found" });
    expect((await pool.query("SELECT count(*) FROM members")).rows[0].count).toBe("0");
  });
});
