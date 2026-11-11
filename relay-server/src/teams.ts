import type { FastifyInstance } from "fastify";
import type { Pool } from "pg";
import { inTransaction } from "@agmsg/reference-server/dist/src/db.js";
import { connectSchema } from "@agmsg/reference-server/dist/src/protocol.js";

export function teams(app: FastifyInstance, pool: Pool): void {
  app.post("/v1/agent-mail/teams", { bodyLimit: 2 * 1024 * 1024 }, async (request, reply) => {
    reply.header("agmsg-protocol-version", "1").header("cache-control", "no-store");
    const parsed = connectSchema.safeParse(request.body);
    if (request.headers["agmsg-protocol-version"] !== "1" ||
        !parsed.success || parsed.data.cipher_profile !== "none") {
      return reply.code(400).send({ error: "expected a protocol 1 plaintext team registration" });
    }
    const input = parsed.data;
    const team = await inTransaction(pool, async client => {
      // Serialize name selection and creation across relay processes.
      await client.query("SELECT pg_advisory_xact_lock(738249113, hashtext($1))", [input.team_name]);
      const existing = await client.query<{ team_id: string }>(
        "SELECT team_id::text FROM teams WHERE team_name=$1 LIMIT 2", [input.team_name],
      );
      if (existing.rows.length > 1) return null;
      const current = existing.rows[0];
      if (!current) {
        await client.query(
          "INSERT INTO teams(team_id,team_name,cipher_profile) VALUES ($1,$2,'none')",
          [input.team_id, input.team_name],
        );
        await client.query(
          `INSERT INTO team_policy_history
           (team_id,policy_revision,effective_from_seq,accepted_envelope_versions,write_allowed_ciphers)
           SELECT team_id,0,1,accepted_envelope_versions,write_allowed_ciphers FROM teams WHERE team_id=$1`,
          [input.team_id],
        );
        const params = [input.team_id, JSON.stringify(input.members)];
        await client.query(
          `INSERT INTO member_identity_history(team_id,member_id,name)
           SELECT $1, member_id, name FROM jsonb_to_recordset($2::jsonb) AS m(member_id uuid,name text)`,
          params,
        );
        await client.query(
          `INSERT INTO members(team_id,member_id,name)
           SELECT $1, member_id, name FROM jsonb_to_recordset($2::jsonb) AS m(member_id uuid,name text)`,
          params,
        );
      }
      const identity = await client.query<{ server_instance_id: string }>(
        "SELECT server_instance_id::text FROM server_metadata WHERE singleton=TRUE",
      );
      const server = identity.rows[0];
      if (!server) throw new Error("relay identity is missing");
      return {
        protocol_version: 1, server_instance_id: server.server_instance_id,
        team_id: current?.team_id ?? input.team_id, team_name: input.team_name,
      };
    });
    if (!team) return reply.code(409).send({ error: "multiple teams already use this name" });
    return team;
  });
}
