import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { Pool } from "pg";
import { afterEach, describe, expect, it } from "vitest";
import { createApp } from "@agmsg/reference-server/dist/src/app.js";
import type { Config } from "@agmsg/reference-server/dist/src/config.js";
import { createRelayApp } from "../src/app.js";

const config: Config = {
  databaseUrl: "postgres://unused",
  host: "127.0.0.1", port: 8787, logLevel: "silent", retentionMaxLiveMessages: null,
};
const headers = {
  "content-type": "application/json",
  "agmsg-protocol-version": "1",
  "agmsg-team-id": "018f3f7e-0000-7000-8000-000000000001",
};
const apps: Array<{ close: () => Promise<void> }> = [];
const pools: Pool[] = [];
function make(factory = createRelayApp) {
  const pool = new Pool({ connectionString: config.databaseUrl });
  pools.push(pool);
  const app = factory(pool, config);
  apps.push(app);
  return app;
}
afterEach(async () => {
  for (const app of apps.splice(0)) await app.close();
  for (const pool of pools.splice(0)) await pool.end();
});

describe("official data plane inside the relay host", () => {
  it("preserves the public protocol routes", async () => {
    const app = make();
    await app.ready();
    for (const [method, url] of [
      ["GET", "/v1/health"], ["GET", "/v1/capabilities"], ["GET", "/v1/members"],
      ["GET", "/v1/messages"], ["POST", "/v1/messages"], ["POST", "/v1/read-state/sync"],
      ["POST", "/v1/connect"], ["GET", "/v1/teams"], ["GET", "/v1/teams/:teamId"],
      ["GET", "/v1/teams/:teamId/messages"],
    ] as const) expect(app.hasRoute({ method, url }), `${method} ${url}`).toBe(true);
  });

  it("matches standalone protocol validation, framing and limits", async () => {
    const original = make(createApp);
    const wrapped = make();
    for (const request of [
      { method: "GET", url: "/v1/messages?after=0" },
      { method: "GET", url: "/v1/health", headers: { "agmsg-team-id": "invalid" } },
      { method: "POST", url: "/v1/connect", headers, payload: '{"team_id":"one","team_id":"two"}' },
      { method: "POST", url: "/v1/messages", headers, payload: { messages: [] } },
      { method: "POST", url: "/v1/messages", headers, payload: { pad: "x".repeat(1100 * 1024) } },
      { method: "POST", url: "/v1/messages", headers, payload: "x".repeat(3 * 1024 * 1024) },
    ] as const) {
      const [a, b] = await Promise.all([original.inject(request), wrapped.inject(request)]);
      expect(b.statusCode).toBe(a.statusCode);
      expect(b.json()).toEqual(a.json());
      expect(b.headers["agmsg-protocol-version"]).toBe("1");
    }
  });

  it("does not leak protocol parsing, headers or errors into sibling routes", async () => {
    const app = make();
    app.setErrorHandler((_error, _request, reply) => { void reply.code(418).send({ host: "error" }); });
    void app.register(async (scope) => {
      scope.post("/extensions/echo", async (request) => request.body);
      scope.get("/extensions/error", async () => { throw new Error("host error"); });
    });
    const host = await app.inject({
      method: "POST", url: "/extensions/echo", headers: { "content-type": "application/json" },
      payload: '{"value":1,"value":2}',
    });
    expect(host.statusCode).toBe(200);
    expect(host.json()).toEqual({ value: 2 });
    expect(host.headers).not.toHaveProperty("agmsg-protocol-version");
    const err = await app.inject({ method: "GET", url: "/extensions/error" });
    expect(err.statusCode).toBe(418);
    expect(err.json()).toEqual({ host: "error" });
    const protocol = await app.inject({ method: "POST", url: "/v1/messages", headers, payload: { messages: [] } });
    expect(protocol.statusCode).toBe(400);
    expect(protocol.json().error.code).toBe("invalid-request");
    const missing = await app.inject({ method: "GET", url: "/missing" });
    expect(missing.statusCode).toBe(404);
    expect(missing.headers).not.toHaveProperty("agmsg-protocol-version");
  });

  it("fails startup cleanly without printing environment credentials", () => {
    const result = spawnSync(process.execPath, [fileURLToPath(new URL("../dist/src/main.js", import.meta.url))], {
      env: { ...process.env, DATABASE_URL: "", AGENT_MAIL_TEST_SECRET: "must-never-print" },
      encoding: "utf8", timeout: 5000,
    });
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("Relay server startup failed");
    expect(result.stdout + result.stderr).not.toContain("must-never-print");
  });
});
