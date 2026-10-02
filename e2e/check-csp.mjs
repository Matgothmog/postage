// Loads the built site (crates/web/dist) with the REAL bridge bundle (Privy
// island and IDKit, no mock) in headless Firefox, served with vercel.json's
// headers, and fails on any Content Security Policy violation.
//
//   node e2e/check-csp.mjs
//
// Privy cannot reach auth.privy.io with a dummy app id and no network, so this
// proves what the policy lets the page load and run (the wasm, the bridge and
// its chunks, IDKit's wasm, fonts, css, Privy's injected styles), not the live
// third-party calls. Those hosts come from Privy's published CSP guide and
// need a look at the staging browser console.
import { existsSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { headerRules, headersFor } from "./headers.mjs";
import { sleep, startBrowser } from "./webdriver.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const config = JSON.parse(readFileSync(join(repo, "vercel.json"), "utf8"));
const dist = join(repo, config.outputDirectory);
const rules = headerRules(repo);
const types = { ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".woff2": "font/woff2", ".ico": "image/x-icon", ".html": "text/html; charset=utf-8" };
const RECORDER = `document.addEventListener("securitypolicyviolation", (e) => {
  const seen = JSON.parse(sessionStorage.getItem("csp") ?? "[]");
  seen.push(e.violatedDirective + " blocked " + (e.blockedURI || "inline") + " at " + e.sourceFile + ":" + e.lineNumber);
  sessionStorage.setItem("csp", JSON.stringify(seen));
});`;

const server = createServer((request, response) => {
  const path = new URL(request.url, "http://x").pathname;
  const send = (status, type, body) => {
    const headers = { "content-type": type };
    for (const { key, value } of headersFor(rules, path)) headers[key] = value;
    response.writeHead(status, headers);
    response.end(body);
  };
  if (path === "/__csp-recorder.js") return send(200, types[".js"], RECORDER);
  const file = join(dist, path);
  if (path !== "/" && file.startsWith(dist) && existsSync(file) && extname(file) !== "") {
    return send(200, types[extname(file)] ?? "application/octet-stream", readFileSync(file));
  }
  const rewritten = config.rewrites.some(({ source }) => new RegExp(`^${source}$`).test(path));
  if (!rewritten) return send(404, "text/plain", "not found");
  const html = readFileSync(join(dist, "index.html"), "utf8").replace("<head>", '<head><script src="/__csp-recorder.js"></script>');
  send(200, types[".html"], html);
});
await new Promise((ready) => server.listen(0, "127.0.0.1", ready));
const origin = `http://127.0.0.1:${server.address().port}`;

const browser = await startBrowser();
let failures = 0;
const report = (what, ok, detail = "") => {
  if (!ok) failures += 1;
  console.log(`${ok ? "PASS" : "FAIL"}  ${what}${detail ? `  [${detail}]` : ""}`);
};
try {
  for (const path of ["/", "/c/aaaaaaaaaaaaaaaaaaaaaaaa", "/network"]) {
    await browser.goto(`${origin}${path}`);
    await browser.waitForText(/\S/);
    await sleep(2500);
    const state = await browser.run(`return {
      wasmApp: typeof window.wasmBindings === "object",
      bridge: typeof globalThis.__postageBridge === "object",
      privyHost: Boolean(document.getElementById("privy-island")),
      text: document.body.innerText.slice(0, 80),
    };`);
    report(`${path}: app wasm started, real bridge installed`, state.wasmApp && state.bridge, JSON.stringify(state.text));
  }
  // Instantiate IDKit's wasm through the real bridge. The request itself is
  // refused (dummy rp signature / no network); what matters is that compiling
  // the module is not blocked.
  await browser.run(`
    const now = Math.floor(Date.now() / 1000);
    const request = { config: { app_id: "app_dummy", action: "a", environment: "sandbox", allow_legacy_proofs: true,
      rp_context: { rp_id: "rp_0123456789abcdef", nonce: "0x" + "11".repeat(32), created_at: now, expires_at: now + 300, signature: "0x" + "22".repeat(65), action: "a" } }, signal: "t" };
    window.__idkit = "pending";
    globalThis.__postageBridge.idkit.openSelfieCheck(JSON.stringify(request))
      .then(() => (window.__idkit = "opened"), (e) => (window.__idkit = "rejected: " + (e?.message ?? e)));
    fetch("/bridge/idkit_wasm_bg.wasm").then((r) => r.arrayBuffer()).then((b) => WebAssembly.compile(b))
      .then(() => (window.__wasm = "compiled"), (e) => (window.__wasm = "failed: " + e.message));`);
  await sleep(4000);
  const outcome = await browser.run("return [window.__idkit, window.__wasm];");
  report("IDKit's wasm compiles under the policy", outcome[1] === "compiled", outcome[1]);
  console.log(`      openSelfieCheck: ${outcome[0]}`);
  const seen = JSON.parse(await browser.run('return sessionStorage.getItem("csp") ?? "[]";'));
  report("no CSP violation recorded on any page", seen.length === 0, seen.slice(0, 6).join(" | "));
} finally {
  await browser.close();
  server.close();
}
process.exit(failures === 0 ? 0 : 1);
