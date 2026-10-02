# Local end-to-end run

Drives the real browser bundle against the real Axum router and a real libsql
file, with every outside service replaced by a loopback stub. Nothing leaves
the machine, and no real ids or keys are used.

```sh
node e2e/run.mjs [--shots <dir>] [--rebuild-web] [--keep]
```

Needs `cargo`, `trunk` (and `npm` for the bridge), `geckodriver` with headless
Firefox, and Node 23+ (`node:sqlite`). The first run builds `crates/web/dist`
with dummy ids; `--rebuild-web` rebuilds it. Screenshots and `results.json`
land in `--shots` (default `$TMPDIR/postage-e2e-shots`). The exit code is
non-zero if any check fails.

## Pieces

- `crates/server/examples/e2e_server` serves `postage_server::router`, the
  built `crates/web/dist` (SPA fallback, like Vercel), and the stub hub under
  `/__stub/<service>` (Anthropic, Resend, Cloudflare, the mail worker, World,
  The Graph, an Arc JSON-RPC node, and Privy's JWKS in memory). The hub
  records every request; `/__stub/admin/*` exposes them and a few switches
  (confirm the Cloudflare address, advance the server clock).
- `e2e/boot.js` replaces `/bridge/bridge.js` in the served page only. Privy
  cannot run with dummy ids, so it installs the bridge over
  `crates/web/tests/mock_sdk.js`, with identity tokens the server signed.
- `e2e/headers.mjs` is a proxy in front of that server that adds the `headers`
  block of `vercel.json`, so the Content Security Policy is enforced for real. It
  moves the server's inline config script into `/__e2e/config.js` and records
  any CSP violation; `run.mjs` fails if there is one.
- `e2e/check-csp.mjs` serves `crates/web/dist` with the same headers and the
  real (unmocked) Privy/IDKit bundle, and fails on any CSP violation.
- `e2e/webdriver.mjs` is a dependency-free WebDriver client.
- `e2e/run.mjs` builds, starts the server, plays the journeys, checks the
  browser text, database rows and stub-captured requests at each step, then
  runs the concurrency burst and the legacy-database comparison.

Run the server alone:

```sh
cargo run -p postage-server --example e2e_server -- \
    --db /tmp/e2e.db --dist crates/web/dist --repo .
```
