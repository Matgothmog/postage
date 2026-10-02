// Checks vercel.json without contacting Vercel: it must parse, and its rewrites
// must send API calls to the function, app routes to index.html, and leave
// static assets alone. Mirrors Vercel's order: the filesystem wins first, then
// rewrites run top to bottom and the first match applies.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const config = JSON.parse(readFileSync(join(root, "vercel.json"), "utf8"));
const distribution = join(root, config.outputDirectory);

function resolve(path) {
  const relative = path.slice(1);
  if (relative !== "" && existsSync(join(distribution, relative))) {
    return `file ${path}`;
  }
  for (const { source, destination } of config.rewrites) {
    const match = new RegExp(`^${source}$`).exec(path.slice(1) === "" ? "/" : path);
    if (match) return `rewrite ${destination}`;
  }
  return "404";
}

assert.equal(config.functions["api/index.rs"].maxDuration, 120);
assert.equal(resolve("/api/inbox"), "rewrite /api/index");
assert.equal(resolve("/api/challenge/abc"), "rewrite /api/index");
assert.equal(resolve("/c/abc"), "rewrite /index.html");
assert.equal(resolve("/network"), "rewrite /index.html");
assert.equal(resolve("/"), "rewrite /index.html");
assert.equal(resolve("/bridge/bridge.js"), "file /bridge/bridge.js");
assert.equal(resolve("/bridge/idkit_wasm_bg.wasm"), "file /bridge/idkit_wasm_bg.wasm");
assert.equal(resolve("/fonts/Geist-Variable.woff2"), "file /fonts/Geist-Variable.woff2");
// A missing asset must stay a 404 instead of answering with HTML.
assert.equal(resolve("/bridge/missing.js"), "404");
assert.equal(resolve("/gone-12345.wasm"), "404");
console.log("vercel.json routing ok");
