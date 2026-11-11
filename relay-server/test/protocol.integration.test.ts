import { randomBytes, randomUUID } from "node:crypto";
import { Pool } from "pg";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { createApp } from "@agmsg/reference-server/dist/src/app.js";
import type { Config } from "@agmsg/reference-server/dist/src/config.js";
import { migrate } from "@agmsg/reference-server/dist/src/db.js";
import { createRelayApp } from "../src/app.js";

const databaseUrl = process.env.TEST_DATABASE_URL;
const databaseTests = databaseUrl ? describe : describe.skip;

databaseTests("wrapped official protocol with real PostgreSQL", () => {
  const schema = `agent_mail_test_${randomBytes(8).toString("hex")}`;
  const teamId = "019a4000-0000-7000-8000-000000000001";
  const memberA = "019a4000-0000-7000-8000-000000000002";
  const memberB = "019a4000-0000-7000-8000-000000000003";
  const headers = { "agmsg-protocol-version": "1", "agmsg-team-id": teamId };
  let admin: Pool | undefined;
  let pool: Pool | undefined;
  let app: ReturnType<typeof createRelayApp> | undefined;
  let original: ReturnType<typeof createApp> | undefined;
  let created = false;

  beforeAll(async () => {
    admin = new Pool({ connectionString: databaseUrl });
    await admin.query(`CREATE SCHEMA ${schema}`);
    created = true;
    pool = new Pool({ connectionString: databaseUrl, options: `-c search_path=${schema}` });
    await migrate(pool);
    // Existing installations must retain their server identity on restart.
    const before = await pool.query("SELECT server_instance_id FROM server_metadata");
    await migrate(pool);
    expect((await pool.query("SELECT server_instance_id FROM server_metadata")).rows).toEqual(before.rows);
    const config: Config = { databaseUrl: databaseUrl!, host: "127.0.0.1", port: 8787, logLevel: "silent", retentionMaxLiveMessages: null };
    app = createRelayApp(pool, config);
    original = createApp(pool, config);
    await app.ready();
    await original.ready();
    const response = await app.inject({ method: "POST", url: "/v1/connect", headers, payload: {
      team_id: teamId, team_name: "wrapper-test", cipher_profile: "none",
      members: [{ member_id: memberA, name: "mac" }, { member_id: memberB, name: "dell" }],
    } });
    expect(response.statusCode).toBe(200);
    expect(response.json()).toMatchObject({ team_id: teamId, team_name: "wrapper-test" });
  });

  afterAll(async () => {
    try {
      await app?.close();
      await original?.close();
    } finally {
      await pool?.end();
      try {
        if (created) {
          if (!/^agent_mail_test_[0-9a-f]{16}$/.test(schema)) throw new Error("unsafe test schema");
          await admin?.query(`DROP SCHEMA ${schema} CASCADE`);
        }
      } finally { await admin?.end(); }
    }
  });

  async function compare(url: string) {
    const [a, b] = await Promise.all([
      app!.inject({ method: "GET", url, headers }), original!.inject({ method: "GET", url, headers }),
    ]);
    expect(a.statusCode).toBe(200);
    expect(a.statusCode).toBe(b.statusCode);
    expect(a.json()).toEqual(b.json());
    expect(a.headers["agmsg-protocol-version"]).toBe("1");
    return a.json();
  }

  it("preserves health, capabilities, roster and team discovery", async () => {
    expect(await compare("/v1/health")).toMatchObject({ status: "ok", database: "ok", team_id: teamId });
    await compare("/v1/capabilities");
    await compare("/v1/members");
    await compare("/v1/teams?name=wrapper-test");
    await compare(`/v1/teams/${teamId}`);
  });

  it("reuses a team name for concurrent named registrations", async () => {
    const a = "019a4000-0000-7000-8000-000000000011";
    const b = "019a4000-0000-7000-8000-000000000012";
    const publish = (id: string, member: string) => app!.inject({
      method: "POST", url: "/v1/agent-mail/teams", headers,
      payload: { team_id: id, team_name: "concurrent", cipher_profile: "none",
        members: [{ member_id: member, name: `reviewer@${id === a ? "A" : "B"}` }] },
    });
    const [first, second] = await Promise.all([publish(a, memberA), publish(b, memberB)]);
    expect(first.statusCode).toBe(200);
    expect(second.statusCode).toBe(200);
    expect(first.json()).toEqual(second.json());
    const id = first.json().team_id;
    expect([a, b]).toContain(id);
    const resolved = await app!.inject({ method: "GET", url: "/v1/teams?name=concurrent", headers });
    expect(resolved.json().teams).toMatchObject([{ team_id: id }]);
    const roster = await app!.inject({
      method: "GET", url: "/v1/members", headers: { ...headers, "agmsg-team-id": id },
    });
    expect(roster.json().members).toMatchObject([{ member_id: id === a ? memberA : memberB }]);
    const reuse = await app!.inject({
      method: "POST", url: "/v1/agent-mail/teams", headers,
      payload: { team_id: a, team_name: "wrapper-test", cipher_profile: "none", members: [] },
    });
    expect(reuse.json().team_id).toBe(teamId);
  });

  it("round trips envelopes with stable duplicate acknowledgments and rollback on conflicts", async () => {
    const id = randomUUID();
    const text = "中文\n'quoted' body; @dell";
    const envelope = { v: 1, cipher: "none", key_id: null, blob: Buffer.from(JSON.stringify({
      body: text, created_at: "2026-11-03T00:00:00.000000Z", from_agent: "mac", to_agent: "dell",
    })).toString("base64") };
    const send = (messages: unknown[]) => app!.inject({ method: "POST", url: "/v1/messages", headers, payload: { messages } });
    const first = await send([{ id, envelope }]);
    expect(first.statusCode).toBe(200);
    expect(first.json().acks).toEqual([{ id, server_seq: "1", disposition: "stored" }]);
    const duplicate = await send([{ id, envelope }]);
    expect(duplicate.statusCode).toBe(200);
    expect(duplicate.json().acks).toEqual([{ id, server_seq: "1", disposition: "duplicate" }]);
    const conflict = await send([
      { id: randomUUID(), envelope },
      { id, envelope: { ...envelope, blob: Buffer.from("different").toString("base64") } },
    ]);
    expect(conflict.statusCode).toBe(409);
    const page = await compare("/v1/messages?after=0&limit=1");
    expect(page.messages).toHaveLength(1);
    expect(page.messages[0]).toMatchObject({ id, server_seq: "1", envelope });
    expect(page.next_after).toBe("1");
    expect(page.has_more).toBe(false);
    expect(JSON.parse(Buffer.from(page.messages[0].envelope.blob, "base64").toString()).body).toBe(text);
    expect((await compare("/v1/messages?after=1")).messages).toHaveLength(0);
  });

  it("preserves read-state updates and rejects cross-team access", async () => {
    const response = await app!.inject({ method: "POST", url: "/v1/read-state/sync", headers, payload: {
      updates: [{ member_id: memberB, server_seq: "1", exact_wire_ids: [] }], page_after: null, page_limit: 100,
    } });
    expect(response.statusCode).toBe(200);
    const readRequest = { method: "POST", url: "/v1/read-state/sync", headers, payload: { updates: [], page_after: null, page_limit: 100 } } as const;
    const [a, b] = await Promise.all([app!.inject(readRequest), original!.inject(readRequest)]);
    expect(a.json()).toEqual(b.json());
    const missing = await app!.inject({ method: "GET", url: "/v1/messages?after=0", headers: {
      ...headers, "agmsg-team-id": "019a4000-0000-7000-8000-000000000099",
    } });
    expect(missing.statusCode).toBe(404);
  });
});
