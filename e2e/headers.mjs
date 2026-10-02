// A reverse proxy that serves the e2e server the way Vercel would: every
// response carries the `headers` block of vercel.json, so the Content Security
// Policy is enforced against the real bundle in the browser.
//
// One adaptation is needed for a test-only reason. The e2e server writes its
// per-run config into the page as an inline <script>, which the policy
// rightly forbids. The proxy moves that script into /__e2e/config.js (same
// origin) and prepends a listener that records every CSP violation in
// sessionStorage, where run.mjs reads them back. The policy itself is never
// loosened.
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";

/** Vercel's `source` patterns are path-to-regexp; vercel.json only uses `/(.*)` and `/prefix/(.*)`. */
export function headerRules(repo) {
  const config = JSON.parse(readFileSync(join(repo, "vercel.json"), "utf8"));
  return config.headers.map(({ source, headers }) => ({
    matches: (path) => new RegExp(`^${source}$`).test(path),
    headers,
  }));
}

/** The headers Vercel would add to a response for `path`, in rule order. */
export function headersFor(rules, path) {
  const merged = new Map();
  for (const rule of rules) {
    if (!rule.matches(path)) continue;
    for (const { key, value } of rule.headers) merged.set(key.toLowerCase(), { key, value });
  }
  return [...merged.values()];
}

const INLINE_CONFIG = /<script>(window\.__E2E = [\s\S]*?;)<\/script>/;
const VIOLATION_RECORDER = `
document.addEventListener("securitypolicyviolation", (event) => {
  try {
    const seen = JSON.parse(sessionStorage.getItem("e2e.csp") ?? "[]");
    seen.push(event.violatedDirective + " blocked " + (event.blockedURI || "inline") + " at " + event.sourceFile + ":" + event.lineNumber);
    sessionStorage.setItem("e2e.csp", JSON.stringify(seen));
  } catch {}
});
`;

export async function startHeaderProxy({ target, repo }) {
  const rules = headerRules(repo);
  let configScript = "";
  const server = createServer(async (request, response) => {
    try {
      const path = new URL(request.url, "http://proxy").pathname;
      if (path === "/__e2e/config.js") {
        response.writeHead(200, { "content-type": "text/javascript", ...Object.fromEntries(headersFor(rules, path).map((h) => [h.key, h.value])) });
        response.end(configScript);
        return;
      }
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const body = chunks.length > 0 ? Buffer.concat(chunks) : undefined;
      const forwarded = Object.fromEntries(
        Object.entries(request.headers).filter(([name]) => !["host", "connection", "content-length"].includes(name))
      );
      const upstream = await fetch(`${target}${request.url}`, { method: request.method, headers: forwarded, body, redirect: "manual" });
      const headers = {};
      upstream.headers.forEach((value, name) => {
        if (!["content-encoding", "content-length", "transfer-encoding", "connection"].includes(name)) headers[name] = value;
      });
      for (const { key, value } of headersFor(rules, path)) headers[key] = value;
      let payload = Buffer.from(await upstream.arrayBuffer());
      if ((headers["content-type"] ?? "").startsWith("text/html")) {
        const html = payload.toString("utf8");
        const inline = INLINE_CONFIG.exec(html);
        if (inline) {
          const origin = `http://${request.headers.host}`;
          configScript = VIOLATION_RECORDER + inline[1].replaceAll(target, origin) + "\n";
          payload = Buffer.from(html.replace(INLINE_CONFIG, '<script src="/__e2e/config.js"></script>'));
        }
      }
      response.writeHead(upstream.status, headers);
      response.end(payload);
    } catch (error) {
      response.writeHead(502, { "content-type": "text/plain" });
      response.end(`proxy error: ${error.message}`);
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  return { origin, rules, stop: () => server.close() };
}
