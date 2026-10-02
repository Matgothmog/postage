// Checks vercel.json without contacting Vercel: it must parse, its rewrites
// must send API calls to the function, app routes to index.html, and leave
// static assets alone, and its headers must reach every route. Mirrors Vercel's order: the filesystem wins first, then
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

// Headers: every rule whose source matches the path applies, later rules
// overriding earlier ones for the same header name.
function headersOf(path) {
  const applied = new Map();
  for (const { source, headers } of config.headers) {
    if (!new RegExp(`^${source}$`).test(path)) continue;
    for (const { key, value } of headers) applied.set(key.toLowerCase(), value);
  }
  return applied;
}
for (const path of ["/", "/c/x", "/network", "/api/x"]) {
  const headers = headersOf(path);
  assert.equal(headers.get("x-content-type-options"), "nosniff", path);
  assert.equal(headers.get("referrer-policy"), "no-referrer", path);
  assert.equal(headers.get("x-frame-options"), "DENY", path);
  assert.match(headers.get("permissions-policy") ?? "", /camera=\(\)/, path);
  const policy = headers.get("content-security-policy") ?? "";
  const directive = (name) => policy.split(";").map((part) => part.trim()).find((part) => part.startsWith(`${name} `));
  assert.equal(directive("default-src"), "default-src 'self'", path);
  assert.equal(directive("frame-ancestors"), "frame-ancestors 'none'", path);
  assert.equal(directive("base-uri"), "base-uri 'none'", path);
  assert.equal(directive("object-src"), "object-src 'none'", path);
  assert.match(directive("script-src") ?? "", /'wasm-unsafe-eval'/, path);
  // Inline and eval script would defeat the policy; styles are the one allowance.
  assert.doesNotMatch(directive("script-src") ?? "", /'unsafe-(inline|eval)'/, path);
}
assert.equal(headersOf("/api/x").get("cache-control"), "no-store");
assert.equal(headersOf("/network").get("cache-control"), undefined);
console.log("vercel.json routing and headers ok");
