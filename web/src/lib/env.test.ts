import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import { identityMode } from "./env";

const ORIGINAL_IDENTITY_MODE = process.env.IDENTITY_MODE;

afterEach(() => {
  if (ORIGINAL_IDENTITY_MODE === undefined) delete process.env.IDENTITY_MODE;
  else process.env.IDENTITY_MODE = ORIGINAL_IDENTITY_MODE;
});

test("defaults to live when IDENTITY_MODE is unset", () => {
  delete process.env.IDENTITY_MODE;
  assert.equal(identityMode(), "live");
});

test("defaults to live when IDENTITY_MODE is the empty string", () => {
  process.env.IDENTITY_MODE = "";
  assert.equal(identityMode(), "live");
});

test("returns live for the exact value \"live\"", () => {
  process.env.IDENTITY_MODE = "live";
  assert.equal(identityMode(), "live");
});

test("returns mock for the explicit value \"mock\"", () => {
  process.env.IDENTITY_MODE = "mock";
  assert.equal(identityMode(), "mock");
});

test("rejects a capitalised near-miss instead of silently resolving to live", () => {
  process.env.IDENTITY_MODE = "Live";
  assert.throws(() => identityMode(), /IDENTITY_MODE/);
});

test("rejects an all-caps near-miss instead of silently resolving to live", () => {
  process.env.IDENTITY_MODE = "LIVE";
  assert.throws(() => identityMode(), /IDENTITY_MODE/);
});

test("rejects a trailing-space near-miss instead of silently resolving to live", () => {
  process.env.IDENTITY_MODE = "live ";
  assert.throws(() => identityMode(), /IDENTITY_MODE/);
});

test("rejects an unrelated value instead of silently resolving to live", () => {
  process.env.IDENTITY_MODE = "production";
  assert.throws(() => identityMode(), /IDENTITY_MODE/);
});

test("names both the variable and the offending value in the thrown error", () => {
  process.env.IDENTITY_MODE = "Live";
  assert.throws(() => identityMode(), (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.match(error.message, /IDENTITY_MODE/);
    assert.match(error.message, /"Live"/);
    return true;
  });
});
