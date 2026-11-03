import { cp, rm } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { checkUpstream } from "./check-upstream.mjs";

const root = fileURLToPath(new URL("../../", import.meta.url));
const require = createRequire(import.meta.url);
const tsc = join(dirname(require.resolve("typescript/package.json")), "bin/tsc");
function compile(project) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [tsc, "-p", project], { cwd: root, stdio: "inherit" });
    child.once("error", reject);
    child.once("close", (code) => code === 0 ? resolve() : reject(new Error(`TypeScript build failed (${code})`)));
  });
}
await checkUpstream();
await rm(join(root, "vendor/agmsg/server/dist"), { recursive: true, force: true });
await compile("vendor/agmsg/server/tsconfig.build.json");
await cp(join(root, "vendor/agmsg/server/migrations"), join(root, "vendor/agmsg/server/dist/migrations"), { recursive: true });
await rm(join(root, "relay-server/dist"), { recursive: true, force: true });
await compile("relay-server/tsconfig.build.json");
