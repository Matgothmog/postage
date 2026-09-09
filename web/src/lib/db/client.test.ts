import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";

// A directory of its own, like every other suite that touches the database —
// a shared path plus `reset()` means two runs on one machine truncate each
// other's tables mid-test.
const workspace = mkdtempSync(join(tmpdir(), "postage-db-client-"));
process.env.DATABASE_URL = `file:${join(workspace, "test.db")}`;
process.env.DATABASE_AUTH_TOKEN = "";

const { all, reset, run } = await import("./client");

after(() => {
  rmSync(workspace, { recursive: true, force: true });
});

// Next's own types declare `NODE_ENV` `readonly` (it is set for us by `next
// build`/`next start`, never by application code) — true everywhere but this
// one test, which has to forge the production build's own signal to prove
// the guard reads it.
function setNodeEnv(value: string | undefined): void {
  (process.env as { NODE_ENV?: string }).NODE_ENV = value;
}

test("reset() is refused when NODE_ENV is production, and leaves data untouched", async () => {
  await run(`CREATE TABLE IF NOT EXISTS reset_guard_probe (id INTEGER)`);
  await run(`INSERT INTO reset_guard_probe (id) VALUES (1)`);

  const originalNodeEnv = process.env.NODE_ENV;
  setNodeEnv("production");
  try {
    await assert.rejects(() => reset(), /refused in production/);
  } finally {
    setNodeEnv(originalNodeEnv);
  }

  const rows = await all<{ id: number }>(`SELECT id FROM reset_guard_probe`);
  assert.equal(rows.length, 1, "the guard must throw before any table is touched");
});

test("reset() empties every table outside production", async () => {
  await run(`CREATE TABLE IF NOT EXISTS reset_guard_probe (id INTEGER)`);
  await run(`INSERT INTO reset_guard_probe (id) VALUES (1)`);

  await reset();

  const rows = await all<{ id: number }>(`SELECT id FROM reset_guard_probe`);
  assert.equal(rows.length, 0);
});
