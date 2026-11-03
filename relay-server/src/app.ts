import Fastify, { type FastifyInstance } from "fastify";
import { dataPlane } from "@agmsg/reference-server/dist/src/app.js";
import type { Config } from "@agmsg/reference-server/dist/src/config.js";
import type { Pool } from "pg";

// The caller owns the pool and closes it after app.close(). No database work
// happens during construction, so embedding and validation can be tested offline.
export function createRelayApp(pool: Pool, config: Config): FastifyInstance {
  const app = Fastify({
    logger: config.logLevel === "silent" ? false : {
      level: config.logLevel,
      redact: { paths: ["req.headers.authorization"], censor: "[REDACTED]" },
    },
  });
  // dataPlane is fastify-plugin: without this ordinary encapsulating plugin,
  // its JSON parser, hooks and error handler would leak into future board/SSE
  // routes. Keep the official URLs unchanged; add our routes as sibling plugins.
  void app.register(async (scope) => {
    await scope.register(dataPlane, { pool, config });
  });
  return app;
}
