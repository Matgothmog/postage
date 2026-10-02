// Trunk post_build hook: moves the inline module script Trunk writes into
// index.html (the wasm loader) into a file of its own. The site's
// Content-Security-Policy (vercel.json) allows `script-src 'self'` without
// 'unsafe-inline', and the loader embeds content-hashed file names, so a CSP
// hash for it could not be kept in a static header.
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const staging = process.env.TRUNK_STAGING_DIR;
if (!staging) {
  console.error("externalize-init: TRUNK_STAGING_DIR is not set");
  process.exit(1);
}

const pagePath = join(staging, "index.html");
const page = readFileSync(pagePath, "utf8");
const inline = /<script type="module">([\s\S]*?)<\/script>/g;

let moved = 0;
const rewritten = page.replace(inline, (_whole, source) => {
  const digest = createHash("sha256").update(source).digest("hex").slice(0, 16);
  const name = `init-${digest}.js`;
  writeFileSync(join(staging, name), source.trim() + "\n");
  moved += 1;
  return `<script type="module" src="/${name}"></script>`;
});

if (moved !== 1) {
  console.error(`externalize-init: expected one inline module script in index.html, found ${moved}`);
  process.exit(1);
}
if (/<script(?![^>]*\bsrc=)[^>]*>/.test(rewritten)) {
  console.error("externalize-init: index.html still has an inline script");
  process.exit(1);
}
writeFileSync(pagePath, rewritten);
