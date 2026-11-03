import { spawn } from "node:child_process";
import { mkdir, symlink, lstat } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { checkUpstream } from "./check-upstream.mjs";

await checkUpstream();
const require = createRequire(import.meta.url);
const upstream = fileURLToPath(new URL("../../vendor/agmsg/server/", import.meta.url));
// The unmodified upstream provisioning test invokes node_modules/tsx directly.
// npm workspaces hoists it, so supply a generated test-only local link and run
// with the upstream server as cwd. No upstream source edits are needed.
const localModules = join(upstream, "node_modules");
await mkdir(localModules, { recursive: true });
try {
  await lstat(join(localModules, "tsx"));
} catch (error) {
  if (error.code !== "ENOENT") throw error;
  await symlink(dirname(require.resolve("tsx/package.json")), join(localModules, "tsx"), process.platform === "win32" ? "junction" : "dir");
}
const vitest = join(dirname(require.resolve("vitest/package.json")), "vitest.mjs");
const child = spawn(process.execPath, [vitest, "run",
  "test/app.plugin.test.ts", "test/config.test.ts", "test/connect.integration.test.ts", "test/server.integration.test.ts",
], { cwd: upstream, stdio: "inherit" });
child.once("error", (error) => { console.error(error.message); process.exitCode = 1; });
child.once("close", (code) => { process.exitCode = code ?? 1; });
