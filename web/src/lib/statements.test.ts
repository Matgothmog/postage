import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { VerifyMessageParameters } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { provesWallet } from "./auth";
import { publicClient } from "./client";
import { MAIL_DOMAIN } from "./handle";
import { claimStatement, confirmStatement, readStatement } from "./statements";
import { now } from "./time";

/// Signatures here are real, because the claim being made is about what a
/// wallet actually put its key to. A stubbed verdict would assert that the stub
/// was consulted and nothing about the text underneath it.
const holder = privateKeyToAccount(`0x${"11".repeat(32)}`);

const WALLET = holder.address;

/// Another deployment of this same app: a staging copy, a fork, or anything
/// else that asks the same wallet to sign for a different `MAIL_DOMAIN`. Built
/// by substitution rather than by hand so the domain is the *only* difference
/// between the two texts — which is what makes a refusal below attributable to
/// the domain line and not to some other drift between two hand-written copies.
///
/// It also means a statement carrying no domain at all leaves this a no-op, so
/// the replay tests fail rather than passing vacuously.
const OTHER_DOMAIN = "usepostage.example";

const asOtherDeployment = (statement: string): string =>
  statement.replaceAll(MAIL_DOMAIN, OTHER_DOMAIN);

const realVerifyMessage = publicClient.verifyMessage;

let asked: VerifyMessageParameters[] = [];

/// The same hostile RPC `auth.test.ts` runs against — it says yes to anything.
/// One test below asserts a signature is *accepted*, which an endpoint
/// answering yes to everything would satisfy however wrong the text was.
/// `afterEach` proves nothing reached it.
beforeEach(() => {
  asked = [];
  publicClient.verifyMessage = async (parameters) => {
    asked.push(parameters);
    return true;
  };
});

afterEach(() => {
  publicClient.verifyMessage = realVerifyMessage;

  assert.deepEqual(asked, [], "a signature was settled by the chain rather than locally");
});

/// The wire format, written out by hand the one time it is written out twice.
/// Both ends build this string from this function, so a change here agrees with
/// itself across the repo while silently changing what an already-open browser
/// tab signs. This is the pin that makes that a failing test instead.
test("readStatement names the deployment alongside the wallet and the timestamp", () => {
  assert.equal(
    readStatement(WALLET, 1_757_332_800),
    [
      "Postage: read my inbox",
      `Domain: ${MAIL_DOMAIN}`,
      `Wallet: ${WALLET.toLowerCase()}`,
      "Issued: 1757332800",
    ].join("\n")
  );
});

/// The asymmetry this closes. `claimStatement` and `confirmStatement` have
/// always carried the domain inside `postageAddress(handle)`; `readStatement`
/// named only a wallet and a time, so a signature over it was good on every
/// deployment that wallet ever touched.
test("every statement a wallet is asked to sign names the deployment asking", () => {
  const issuedAt = 1_757_332_800;

  assert.ok(readStatement(WALLET, issuedAt).includes(MAIL_DOMAIN), "readStatement is unbound");
  assert.ok(
    claimStatement("demo", "reader@example.com", WALLET, issuedAt).includes(MAIL_DOMAIN),
    "claimStatement is unbound"
  );
  assert.ok(
    confirmStatement("demo", WALLET, issuedAt).includes(MAIL_DOMAIN),
    "confirmStatement is unbound"
  );
});

/// The replay this exists to stop. Reading an inbox discloses the forwarding
/// address behind the handle, so a signature harvested wherever else this
/// wallet signs must not open it here.
test("a read signature collected on another deployment proves nothing here", async () => {
  const issuedAt = now();
  const elsewhere = asOtherDeployment(readStatement(WALLET, issuedAt));

  const signature = await holder.signMessage({ message: elsewhere });

  assert.equal(
    await provesWallet(WALLET, issuedAt, signature, (at) => readStatement(WALLET, at)),
    false
  );
});

/// And the same trip the other way round, because binding one deployment's
/// statement is worthless if it also refuses its own.
test("a read signature collected on this deployment still proves the wallet", async () => {
  const issuedAt = now();

  const signature = await holder.signMessage({ message: readStatement(WALLET, issuedAt) });

  assert.equal(
    await provesWallet(WALLET, issuedAt, signature, (at) => readStatement(WALLET, at)),
    true
  );
});

/// The behaviour `readStatement` is being brought into line with, asserted
/// against the same foreign deployment, so the three statements are now refused
/// for the same reason rather than two of them by accident of carrying a handle.
test("a confirm signature collected on another deployment proves nothing here", async () => {
  const issuedAt = now();
  const elsewhere = asOtherDeployment(confirmStatement("demo", WALLET, issuedAt));

  const signature = await holder.signMessage({ message: elsewhere });

  assert.equal(
    await provesWallet(WALLET, issuedAt, signature, (at) => confirmStatement("demo", WALLET, at)),
    false
  );
});
