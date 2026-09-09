import assert from "node:assert/strict";
import { test } from "node:test";

import {
  MIN_POLL_SECONDS,
  POLL_SAFETY_MARGIN_SECONDS,
  RP_CONTEXT_TTL_SECONDS,
  pollTimeoutMs,
  type RpContextWindow,
} from "./rp-context";

const CREATED_AT = 1_700_000_000;

function windowOf(ttlSeconds: number): RpContextWindow {
  return { created_at: CREATED_AT, expires_at: CREATED_AT + ttlSeconds };
}

/// Simulates a runtime value that reached `pollTimeoutMs` without passing
/// through `isRpContext` in `world-id.ts` — the only path a malformed shape
/// can take, since TypeScript itself would reject these object literals
/// against `RpContextWindow` directly.
function malformedWindow(value: Record<string, unknown>): RpContextWindow {
  return value as unknown as RpContextWindow;
}

test("the poll window closes before the signature it was issued under expires", () => {
  const timeout = pollTimeoutMs(windowOf(RP_CONTEXT_TTL_SECONDS));

  assert.equal(timeout, (RP_CONTEXT_TTL_SECONDS - POLL_SAFETY_MARGIN_SECONDS) * 1000);
  assert.ok(
    timeout < RP_CONTEXT_TTL_SECONDS * 1000,
    "a sender must give up with a plain timeout before World reports rp_signature_expired"
  );
});

test("measures the window from the signature's own timestamps, not any wall clock", () => {
  const decadeLater = { created_at: CREATED_AT + 315_360_000, expires_at: CREATED_AT + 315_360_300 };

  assert.equal(pollTimeoutMs(windowOf(300)), pollTimeoutMs(decadeLater));
});

test("never hands a poll loop a window that is already over", () => {
  const timeout = pollTimeoutMs(windowOf(POLL_SAFETY_MARGIN_SECONDS - 1));

  assert.ok(timeout > 0, "a zero or negative timeout fails the request instantly");
});

test("floors a context missing both timestamps to the minimum poll window, not NaN", () => {
  const timeout = pollTimeoutMs(malformedWindow({}));

  assert.equal(timeout, MIN_POLL_SECONDS * 1000);
});

test("floors a context missing expires_at to the minimum poll window, not NaN", () => {
  const timeout = pollTimeoutMs(malformedWindow({ created_at: CREATED_AT }));

  assert.equal(timeout, MIN_POLL_SECONDS * 1000);
});

test("floors a context missing created_at to the minimum poll window, not NaN", () => {
  const timeout = pollTimeoutMs(malformedWindow({ expires_at: CREATED_AT }));

  assert.equal(timeout, MIN_POLL_SECONDS * 1000);
});

test("floors a context with null timestamps to the minimum poll window", () => {
  const timeout = pollTimeoutMs(malformedWindow({ created_at: null, expires_at: null }));

  assert.equal(timeout, MIN_POLL_SECONDS * 1000);
});

test("computes the real window when timestamps arrive as numeric strings", () => {
  const timeout = pollTimeoutMs(
    malformedWindow({ created_at: String(CREATED_AT), expires_at: String(CREATED_AT + RP_CONTEXT_TTL_SECONDS) })
  );

  assert.equal(timeout, (RP_CONTEXT_TTL_SECONDS - POLL_SAFETY_MARGIN_SECONDS) * 1000);
});

test("never returns NaN for any malformed timestamp shape", () => {
  const malformed = [{}, { created_at: CREATED_AT }, { expires_at: CREATED_AT }, { created_at: null, expires_at: null }];

  for (const shape of malformed) {
    assert.equal(Number.isNaN(pollTimeoutMs(malformedWindow(shape))), false);
  }
});
