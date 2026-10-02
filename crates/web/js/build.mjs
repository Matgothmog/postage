// Bundles src/entry.js into dist/bridge.js (one ESM entry) and copies IDKit's
// wasm next to it: idkit-core fetches it from
// `new URL("idkit_wasm_bg.wasm", import.meta.url)`, i.e. beside the bundle.
//
// Code splitting stays on: Privy dynamically imports its modal screens and
// wallet connectors, and without splitting all of them would be inlined into
// one ~5 MiB entry the page must load before the app starts. Split, the entry
// statically pulls in only the chunks it needs to boot (reported as "eager"
// below); the rest are fetched from dist/chunks/ when a screen first opens.

import { build } from "esbuild";
import { copyFile, mkdir, readFile, readdir, rm, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { gzipSync } from "node:zlib";

const root = dirname(fileURLToPath(import.meta.url));
const outdir = join(root, "dist");
const idkitWasm = join(root, "node_modules/@worldcoin/idkit-core/dist/idkit_wasm_bg.wasm");

await rm(outdir, { recursive: true, force: true });
await mkdir(outdir, { recursive: true });

await build({
  entryPoints: [join(root, "src/entry.js")],
  outdir,
  entryNames: "bridge",
  chunkNames: "chunks/[name]-[hash]",
  splitting: true,
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2022",
  minify: true,
  legalComments: "none",
  logLevel: "warning",
  define: { "process.env.NODE_ENV": '"production"', global: "globalThis" },
});
await copyFile(idkitWasm, join(outdir, "idkit_wasm_bg.wasm"));

const kib = (bytes) => `${(bytes / 1024).toFixed(0)} KiB`;

/// Everything `file` statically imports, transitively: what the browser must
/// fetch before the bridge has installed itself.
async function eagerGraph(file, seen = new Set()) {
  if (seen.has(file)) return seen;
  seen.add(file);
  const source = await readFile(file, "utf8");
  for (const [, specifier] of source.matchAll(/(?:^|[;}\s])import\s*(?:[\w${},\s*]+from\s*)?"(\.[^"]+)"/g)) {
    await eagerGraph(resolve(dirname(file), specifier), seen);
  }
  return seen;
}

let eagerRaw = 0;
let eagerGzip = 0;
const eager = await eagerGraph(join(outdir, "bridge.js"));
for (const file of eager) {
  const bytes = await readFile(file);
  eagerRaw += bytes.length;
  eagerGzip += gzipSync(bytes).length;
}
console.log(`eager: bridge.js + ${eager.size - 1} chunks, ${kib(eagerRaw)} (${kib(eagerGzip)} gzip)`);

const chunks = await readdir(join(outdir, "chunks"));
const chunkSizes = await Promise.all(chunks.map((file) => stat(join(outdir, "chunks", file))));
const chunkTotal = chunkSizes.reduce((sum, { size }) => sum + size, 0);
console.log(`all chunks: ${chunks.length} files, ${kib(chunkTotal)}`);
const { size: wasmSize } = await stat(join(outdir, "idkit_wasm_bg.wasm"));
console.log(`idkit_wasm_bg.wasm (fetched on first Selfie Check): ${kib(wasmSize)}`);
