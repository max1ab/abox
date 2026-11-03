import { loadConfig } from "@agmsg/reference-server/dist/src/config.js";
import { createPool, migrate } from "@agmsg/reference-server/dist/src/db.js";
import { createRelayApp } from "./app.js";
import { migrateCollaboration } from "./collaboration.js";
import { migrateEvents } from "./events.js";

async function start(): Promise<void> {
  const config = loadConfig();
  const pool = createPool(config.databaseUrl);
  let app: ReturnType<typeof createRelayApp> | undefined;
  try {
    await migrate(pool);
    await migrateCollaboration(pool);
    await migrateEvents(pool);
    app = createRelayApp(pool, config);
    await app.listen({ host: config.host, port: config.port });
  } catch (error) {
    try { await app?.close(); } finally { await pool.end(); }
    throw error;
  }
  const runningApp = app;
  let stopping: Promise<void> | undefined;
  const shutdown = () => {
    stopping ??= (async () => {
      try { await runningApp.close(); } finally { await pool.end(); }
    })();
    void stopping.catch(() => { process.exitCode = 1; });
  };
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);
}

try {
  await start();
} catch (error) {
  // A connection string can contain credentials. Never print the environment
  // or arbitrary upstream exception text on the startup failure path.
  console.error(`Relay server startup failed (${error instanceof Error ? error.name : "unknown error"}). Check DATABASE_URL and PostgreSQL availability.`);
  process.exitCode = 1;
}
