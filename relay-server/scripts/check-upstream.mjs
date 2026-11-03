import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { join, resolve } from "node:path";

const root = fileURLToPath(new URL("../../vendor/agmsg/", import.meta.url));
export async function checkUpstream() {
  const manifest = JSON.parse(await readFile(join(root, "UPSTREAM.json"), "utf8"));
  const seen = new Set();
  async function walk(dir, prefix = "") {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const rel = prefix + entry.name;
      if (rel === "UPSTREAM.json" || rel === "server/dist" || rel === "server/node_modules") continue;
      if (entry.isDirectory()) await walk(join(dir, entry.name), rel + "/");
      else {
        if (!entry.isFile()) throw new Error(`unexpected upstream file type: ${rel}`);
        const hash = createHash("sha256").update(await readFile(join(dir, entry.name))).digest("hex");
        if (manifest.sha256[rel] !== hash) throw new Error(`upstream checksum mismatch: ${rel}`);
        seen.add(rel);
      }
    }
  }
  await walk(root);
  for (const name of Object.keys(manifest.sha256)) {
    if (!seen.has(name)) throw new Error(`missing upstream file: ${name}`);
  }
  console.log(`agmsg ${manifest.tag} (${manifest.commit}): ${seen.size} source files verified`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await checkUpstream();
}
