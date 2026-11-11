import type { FastifyInstance } from "fastify";
import type { Pool, PoolClient, Notification } from "pg";
import type { ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";

const channel = "agent_mail_events";
const teamPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// A transactional trigger catches writes from every producer, including the
// unmodified official data plane. ROLLBACK produces no wake; a process crash
// cannot leave a committed message dependent on an in-memory onSend hook.
export async function migrateEvents(pool: Pool): Promise<void> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("SELECT pg_advisory_xact_lock(738249111)");
    await client.query(`CREATE OR REPLACE FUNCTION am_notify_mail() RETURNS trigger LANGUAGE plpgsql AS $$
      BEGIN
        PERFORM pg_notify('agent_mail_events', json_build_object('schema',TG_TABLE_SCHEMA,'team_id',NEW.team_id::text)::text);
        RETURN NEW;
      END;
    $$;
    DROP TRIGGER IF EXISTS am_mail_committed ON messages;
    CREATE TRIGGER am_mail_committed AFTER INSERT ON messages FOR EACH ROW EXECUTE FUNCTION am_notify_mail();`);
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK"); throw error;
  } finally { client.release(); }
}

type Subscriber = { team: string; response: ServerResponse; payload: string; presence: boolean };
type Probe = { team: string; finish: (members: string[]) => void };
class EventHub {
  private client: PoolClient | undefined;
  private starting: Promise<void> | undefined;
  private schema: string | undefined;
  private closed = false;
  private timer: ReturnType<typeof setInterval> | undefined;
  private readonly subscribers = new Set<Subscriber>();
  private readonly probes = new Map<string, Probe>();
  constructor(private readonly pool: Pool) {}
  async start(): Promise<void> {
    if (this.closed) throw new Error("event hub is closing");
    if (this.client) return;
    if (!this.starting) {
      this.starting = this.connect().finally(() => { this.starting = undefined; });
    }
    await this.starting;
  }
  private async connect(): Promise<void> {
    const client = await this.pool.connect();
    try {
      client.on("error", this.failed);
      client.on("notification", this.notified);
      const identity = (await client.query("SELECT current_schema() AS name, current_setting('application_name') AS application")).rows[0];
      this.schema = identity.name;
      await client.query("SELECT set_config('application_name',$1,false)", [`${identity.application}:agent-mail-events`]);
      await client.query(`LISTEN ${channel}`);
      if (this.closed) throw new Error("event hub closed during startup");
      this.client = client;
      // A single shared health probe covers an idle/half-open LISTEN socket.
      // If it fails, end SSE streams so clients reconnect and pull by cursor.
      this.timer = setInterval(() => {
        // pg client.js supports query_timeout on an individual query.
        const probe = { text: "SELECT 1", query_timeout: 5000 };
        void client.query(probe).catch(() => { if (this.client === client) this.failed(); });
      }, 20000);
      this.timer.unref();
    } catch (error) {
      client.off("error", this.failed); client.off("notification", this.notified);
      client.release(true); throw error;
    }
  }
  private readonly failed = () => {
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    const client = this.client; this.client = undefined;
    for (const subscriber of this.subscribers) subscriber.response.destroy();
    this.subscribers.clear();
    for (const probe of this.probes.values()) probe.finish([]);
    if (client) {
      client.off("notification", this.notified);
      client.off("error", this.failed);
      client.on("error", () => {});
      client.release(true);
    }
  };
  private readonly notified = (notice: Notification) => {
    if (notice.channel !== channel || !notice.payload) return;
    let value: unknown;
    try { value = JSON.parse(notice.payload); } catch { return; }
    if (!value || typeof value !== "object" || !("schema" in value) || value.schema !== this.schema || !("team_id" in value)) return;
    for (const subscriber of this.subscribers) {
      if (subscriber.team === value.team_id) this.send(subscriber);
    }
  };
  add(subscriber: Subscriber): () => void {
    if (this.closed || !this.client) throw new Error("event listener unavailable");
    this.subscribers.add(subscriber);
    return () => { this.subscribers.delete(subscriber); };
  }
  send(subscriber: Subscriber, event = "new_messages", payload = subscriber.payload): boolean {
    // No mail bodies or durable cursor travel in this stream. Backpressure
    // closes a slow subscriber instead of accumulating a per-agent queue.
    if (subscriber.response.destroyed || subscriber.response.writableEnded) { this.subscribers.delete(subscriber); return false; }
    if (!subscriber.response.write(`event: ${event}\ndata: ${payload}\n\n`)) {
      this.subscribers.delete(subscriber); subscriber.response.destroy();
      return false;
    }
    return true;
  }
  async available(team: string, member: string | undefined): Promise<string[]> {
    const subscribers = [...this.subscribers].filter(s => s.team === team && s.presence);
    if (this.probes.size + subscribers.length > 512) throw new Error("too many presence probes");
    const responses = await Promise.all(subscribers.map(subscriber => new Promise<string[]>(resolve => {
      const id = randomUUID();
      const finish = (members: string[]) => {
        clearTimeout(timer);
        this.probes.delete(id);
        resolve(members);
      };
      const timer = setTimeout(() => finish([]), 1000);
      this.probes.set(id, { team, finish });
      const payload = JSON.stringify({ ...JSON.parse(subscriber.payload), probe_id: id, member_id: member });
      if (!this.send(subscriber, "presence_probe", payload)) finish([]);
    })));
    return [...new Set(responses.flat())].filter(id => member === undefined || id === member);
  }
  answer(team: string, id: string, members: string[]): void {
    const probe = this.probes.get(id);
    if (probe?.team === team) probe.finish(members);
  }
  async close(): Promise<void> {
    this.closed = true;
    await this.starting?.catch(() => {});
    this.failed();
  }
}

export async function events(app: FastifyInstance, pool: Pool): Promise<void> {
  const hub = new EventHub(pool);
  const identity = async (team: string) => (await pool.query(
    `SELECT 1 AS protocol_version, m.server_instance_id::text, t.team_id::text
     FROM server_metadata m CROSS JOIN teams t WHERE m.singleton=TRUE AND t.team_id=$1`,
    [team],
  )).rows[0];
  app.addHook("preClose", async () => { await hub.close(); });
  app.get<{ Querystring: { member_id?: string } }>("/v1/agent-mail/presence", async (request, reply) => {
    const team = request.headers["agmsg-team-id"];
    const member = request.query.member_id;
    if (request.headers["agmsg-protocol-version"] !== "1" || typeof team !== "string" || !teamPattern.test(team)
        || (member !== undefined && (typeof member !== "string" || !teamPattern.test(member)))) {
      return reply.code(400).send({ error: "invalid presence query" });
    }
    const binding = await identity(team);
    if (!binding) return reply.code(404).send({ error: "team not found" });
    try {
      const members = await hub.available(team, member);
      return reply.header("agmsg-protocol-version", "1").header("cache-control", "no-store").send({ ...binding, members });
    } catch {
      return reply.code(503).send({ error: "presence temporarily unavailable" });
    }
  });
  app.post<{ Params: { id: string }; Body: { members?: unknown } }>(
    "/v1/agent-mail/presence/:id", { bodyLimit: 256 * 1024 }, async (request, reply) => {
      const team = request.headers["agmsg-team-id"];
      const members = request.body?.members;
      if (request.headers["agmsg-protocol-version"] !== "1" || typeof team !== "string" || !teamPattern.test(team)
          || !Array.isArray(members) || !members.every(id => typeof id === "string" && teamPattern.test(id))) {
        return reply.code(400).send({ error: "invalid presence response" });
      }
      hub.answer(team, request.params.id, members);
      return reply.code(204).send();
    },
  );
  app.get("/v1/agent-mail/events", async (request, reply) => {
    const team = request.headers["agmsg-team-id"];
    if (request.headers["agmsg-protocol-version"] !== "1" || typeof team !== "string" || !teamPattern.test(team)) {
      return reply.code(400).send({ error: "expected protocol 1 and a UUIDv7 team ID" });
    }
    try {
      // Subscribe before querying team identity; send an initial hint after
      // adding the HTTP stream. This closes the subscribe/pull startup race.
      await hub.start();
      const binding = await identity(team);
      if (!binding) return reply.code(404).send({ error: "team not found" });
      if (request.raw.destroyed) return;
      const subscriber = { team, response: reply.raw, payload: JSON.stringify(binding),
        presence: request.headers["agent-mail-presence"] === "1" };
      const remove = hub.add(subscriber);
      reply.hijack();
      reply.raw.writeHead(200, {
        "content-type": "text/event-stream; charset=utf-8", "cache-control": "no-cache",
        "connection": "keep-alive", "x-accel-buffering": "no", "agmsg-protocol-version": "1",
      });
      reply.raw.once("close", () => { clearInterval(heartbeat); remove(); });
      const heartbeat = setInterval(() => {
        if (!reply.raw.write(": keepalive\n\n")) reply.raw.destroy();
      }, 15000);
      heartbeat.unref();
      hub.send(subscriber);
    } catch {
      if (!reply.raw.headersSent) return reply.code(503).send({ error: "event listener unavailable; use cursor pull" });
      reply.raw.end();
    }
  });
}
