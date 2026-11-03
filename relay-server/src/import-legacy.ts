import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";
import type { Pool } from "pg";
import { createPool } from "@agmsg/reference-server/dist/src/db.js";
import { migrateCollaboration, validLabel } from "./collaboration.js";

type Post = { id: number; team: string; author: string; body: string; created_at: string };
type Profile = { team: string; agent: string; display_name: string; description: string; updated_at: string };
type Snapshot = { version: 1; posts: Post[]; profiles: Profile[] };
function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
function text(value: unknown): value is string {
  return typeof value === "string" && !value.includes("\0");
}
function validate(value: unknown): asserts value is Snapshot {
  if (!record(value) || value.version !== 1 || !Array.isArray(value.posts) || !Array.isArray(value.profiles)) {
    throw new Error("expected a version 1 export-legacy snapshot");
  }
  const ids = new Set<number>();
  const keys = new Set<string>();
  for (const row of value.posts) {
    if (!record(row) || !Number.isSafeInteger(row.id) || Number(row.id) <= 0 || ids.has(Number(row.id))
      || !validLabel(row.team) || !validLabel(row.author) || !text(row.body) || !row.body.trim()
      || Buffer.byteLength(row.body) > 65536 || !text(row.created_at) || !row.created_at) throw new Error("invalid or duplicate legacy post");
    ids.add(Number(row.id));
  }
  for (const row of value.profiles) {
    if (!record(row) || !validLabel(row.team) || !validLabel(row.agent) || !validLabel(row.display_name)
      || !text(row.description) || Buffer.byteLength(row.description) > 4096 || !text(row.updated_at) || !row.updated_at) throw new Error("invalid legacy profile");
    const key = JSON.stringify([row.team, row.agent]);
    if (keys.has(key)) throw new Error("duplicate legacy profile");
    keys.add(key);
  }
}

// Offline bootstrap only: must run before the new board accepts writes. Existing
// mail tables and server identity are deliberately outside this transaction.
export async function importLegacy(pool: Pool, bytes: Buffer): Promise<"imported" | "already_imported"> {
  const snapshot: unknown = JSON.parse(bytes.toString("utf8"));
  validate(snapshot);
  const digest = createHash("sha256").update(bytes).digest("hex");
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("LOCK TABLE am_board_posts, am_agent_profiles, am_legacy_import IN ACCESS EXCLUSIVE MODE");
    const prior = await client.query("SELECT digest FROM am_legacy_import WHERE singleton=true");
    if (prior.rows.length) {
      if (prior.rows[0].digest !== digest) throw new Error("another snapshot was already imported");
      await client.query("COMMIT");
      return "already_imported";
    }
    const existing = await client.query("SELECT (SELECT count(*) FROM am_board_posts)+(SELECT count(*) FROM am_agent_profiles) AS count");
    if (existing.rows[0].count !== "0") throw new Error("destination board/profiles must be empty");
    for (const row of snapshot.posts) await client.query(
      "INSERT INTO am_board_posts(id,team,author,body,created_at) VALUES ($1,$2,$3,$4,$5)",
      [row.id,row.team,row.author,row.body,row.created_at]);
    for (const row of snapshot.profiles) await client.query(
      "INSERT INTO am_agent_profiles(team,agent,display_name,description,updated_at) VALUES ($1,$2,$3,$4,$5)",
      [row.team,row.agent,row.display_name,row.description,row.updated_at]);
    // ALTER SEQUENCE RESTART is transactional (setval is not).
    const next = snapshot.posts.reduce((max, row) => Math.max(max, row.id), 0) + 1;
    if (!Number.isSafeInteger(next)) throw new Error("post IDs exceed supported range");
    await client.query(`ALTER TABLE am_board_posts ALTER COLUMN id RESTART WITH ${next}`);
    await client.query("INSERT INTO am_legacy_import(singleton,digest) VALUES (true,$1)", [digest]);
    await client.query("COMMIT");
    return "imported";
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally { client.release(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  let pool: Pool | undefined;
  try {
    const file = process.argv[2];
    if (!file || !process.env.DATABASE_URL) throw new Error("usage: DATABASE_URL=... node dist/src/import-legacy.js snapshot.json");
    pool = createPool(process.env.DATABASE_URL);
    await migrateCollaboration(pool);
    console.log(await importLegacy(pool, await readFile(file)));
  } catch {
    console.error("Legacy import failed. Check the snapshot, database connection, and empty destination. No partial import is committed.");
    process.exitCode = 1;
  } finally { await pool?.end(); }
}
