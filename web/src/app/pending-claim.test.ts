import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import type { ClaimProgress } from "./claim-inbox-helpers";
import {
  type ClaimStore,
  PENDING_CLAIM_KEY,
  claimFor,
  claimStore,
  clearPendingClaim,
  pollVerdict,
  readStoredClaim,
  verifyClaimUrl,
  verifyPendingClaim,
  writePendingClaim,
} from "./pending-claim";

const CLAIM: ClaimProgress = {
  handle: "demo",
  destination: "demo@example.com",
  codeVerified: false,
  cloudflareVerified: false,
};

/// Two wallets that sign in on the same browser. The second is the stranger the
/// first one's claim must never be handed to.
const WALLET = "0xAb0000000000000000000000000000000000000a";
const OTHER_WALLET = "0xCd0000000000000000000000000000000000000c";

function fakeStore(seed: Record<string, string> = {}): ClaimStore {
  const items = new Map(Object.entries(seed));
  return {
    getItem: (key) => items.get(key) ?? null,
    setItem: (key, value) => {
      items.set(key, value);
    },
    removeItem: (key) => {
      items.delete(key);
    },
  };
}

/// What a browser configured to refuse site data does: every call throws, not
/// just the first.
function refusingStore(): ClaimStore {
  const refuse = () => {
    throw new Error("storage is disabled");
  };
  return { getItem: refuse, setItem: refuse, removeItem: refuse };
}

/// What `readStoredClaim` followed by `claimFor` does together, which is how
/// every caller reads the slot: find what is there, then ask whether it is
/// this session's to see.
function readFor(store: ClaimStore | null, wallet: string | null): ClaimProgress | null {
  return claimFor(readStoredClaim(store), wallet);
}

const realFetch = globalThis.fetch;

function stubVerifyEndpoint(reply: (url: string) => Response): void {
  globalThis.fetch = (async (input: RequestInfo | URL) => reply(String(input))) as typeof fetch;
}

beforeEach(() => {
  stubVerifyEndpoint(() =>
    Response.json({ codeVerified: false, cloudflareVerified: false, live: false, stalled: false })
  );
});

afterEach(() => {
  globalThis.fetch = realFetch;
});

test("claimStore hands back nothing where there is no localStorage, rather than throwing", () => {
  assert.equal(claimStore(), null);
});

test("readStoredClaim returns nothing when there is no store to read", () => {
  assert.equal(readStoredClaim(null), null);
});

test("readStoredClaim returns nothing when no claim was ever stored", () => {
  assert.equal(readStoredClaim(fakeStore()), null);
});

test("a stored claim is read back whole, so a reload resumes where it left off", () => {
  const store = fakeStore();
  writePendingClaim(store, WALLET, { ...CLAIM, codeVerified: true });
  assert.deepEqual(readFor(store, WALLET), { ...CLAIM, codeVerified: true });
});

test("readStoredClaim returns nothing for stored text that is not JSON", () => {
  assert.equal(readStoredClaim(fakeStore({ [PENDING_CLAIM_KEY]: "not json" })), null);
});

test("readStoredClaim returns nothing for a stored value missing the fields the strip needs", () => {
  const stored = JSON.stringify({ wallet: WALLET, claim: { handle: "demo" } });
  assert.equal(readStoredClaim(fakeStore({ [PENDING_CLAIM_KEY]: stored })), null);
});

test("readStoredClaim returns nothing for a stored value that is not an object at all", () => {
  assert.equal(readStoredClaim(fakeStore({ [PENDING_CLAIM_KEY]: '"demo"' })), null);
});

test("readStoredClaim returns nothing when the store itself refuses to be read", () => {
  assert.equal(readStoredClaim(refusingStore()), null);
});

/// The shape written before claims were scoped to a wallet. It cannot be shown
/// to anybody, because there is nobody it can be proved to belong to.
test("a claim stored in the older unscoped shape is not handed to anyone (H2)", () => {
  const stored = JSON.stringify(CLAIM);
  assert.equal(readStoredClaim(fakeStore({ [PENDING_CLAIM_KEY]: stored })), null);
  assert.equal(readFor(fakeStore({ [PENDING_CLAIM_KEY]: stored }), WALLET), null);
});

test("a claim stored with an empty wallet is not handed to anyone (H2)", () => {
  const stored = JSON.stringify({ wallet: "", claim: CLAIM });
  assert.equal(readStoredClaim(fakeStore({ [PENDING_CLAIM_KEY]: stored })), null);
});

/// The cross-user failure this scoping exists for. Wallet A claims a handle and
/// signs out; wallet B signs in on the same browser. Without the wallet on the
/// record, B was shown A's handle and A's destination address, and B's code
/// POST came back 401 with no way out of the screen.
test("a claim wallet A left behind is never handed to wallet B (H2)", () => {
  const store = fakeStore();
  writePendingClaim(store, WALLET, CLAIM);

  assert.equal(readFor(store, OTHER_WALLET), null, "B must not be shown A's claim");
  assert.deepEqual(readFor(store, WALLET), CLAIM, "A still gets its own claim back");
});

test("claimFor hands back nothing while there is no session to scope the claim to (H2)", () => {
  const store = fakeStore();
  writePendingClaim(store, WALLET, CLAIM);

  assert.equal(readFor(store, null), null);
});

/// Privy hands back a checksummed address and the server answers with a
/// lower-cased one. They are the same wallet, and its owner must not lose their
/// own claim to a difference in capitalisation.
test("a claim is still its owner's when the address comes back cased differently (H2)", () => {
  const store = fakeStore();
  writePendingClaim(store, WALLET.toLowerCase(), CLAIM);

  assert.deepEqual(readFor(store, WALLET.toUpperCase()), CLAIM);
});

test("writePendingClaim records the wallet that made the claim (H2)", () => {
  const store = fakeStore();
  writePendingClaim(store, WALLET, CLAIM);

  assert.deepEqual(readStoredClaim(store), { wallet: WALLET, claim: CLAIM });
});

test("writePendingClaim stores nothing for a session with no wallet yet", () => {
  const store = fakeStore();
  writePendingClaim(store, null, CLAIM);

  assert.equal(readStoredClaim(store), null);
});

test("writePendingClaim on a store that refuses to write is a no-op, not a crash", () => {
  assert.doesNotThrow(() => writePendingClaim(refusingStore(), WALLET, CLAIM));
});

test("writePendingClaim with no store is a no-op, not a crash", () => {
  assert.doesNotThrow(() => writePendingClaim(null, WALLET, CLAIM));
});

test("clearPendingClaim forgets the stored claim", () => {
  const store = fakeStore();
  writePendingClaim(store, WALLET, CLAIM);
  clearPendingClaim(store);
  assert.equal(readStoredClaim(store), null);
});

test("clearPendingClaim tolerates a missing store and one that refuses", () => {
  assert.doesNotThrow(() => clearPendingClaim(null));
  assert.doesNotThrow(() => clearPendingClaim(refusingStore()));
});

/// `isPendingClaim` only ever checked that the handle was a non-empty string,
/// so a handle carrying `&` or `#` used to rewrite the query string it was
/// pasted into. One builder, url-encoding once, for both callers.
test("verifyClaimUrl encodes a handle that would otherwise rewrite the query string (low)", () => {
  assert.equal(verifyClaimUrl("a&b=c"), "/api/inbox/verify?handle=a%26b%3Dc");
  assert.equal(verifyClaimUrl("a b"), "/api/inbox/verify?handle=a%20b");
  assert.equal(verifyClaimUrl("demo"), "/api/inbox/verify?handle=demo");
});

/// The distinction the poll used to lose: "the server says this is gone" is not
/// the same answer as "I could not reach the server", and only the first is a
/// reason to stop showing the claim.
test("pollVerdict reads a 404 as the server saying the claim is gone (M6)", () => {
  assert.equal(pollVerdict(404), "gone");
});

test("pollVerdict reads an answer as something to apply (M6)", () => {
  assert.equal(pollVerdict(200), "read");
  assert.equal(pollVerdict(299), "read");
});

test("pollVerdict reads every other failure as a reason to wait, not to forget the claim (M6)", () => {
  assert.equal(pollVerdict(503), "wait");
  assert.equal(pollVerdict(500), "wait");
  assert.equal(pollVerdict(429), "wait");
  assert.equal(pollVerdict(400), "wait");
  assert.equal(pollVerdict(301), "wait");
});

test("verifyPendingClaim asks the poll endpoint about the stored handle, url-encoded", async () => {
  let asked: string | null = null;
  stubVerifyEndpoint((url) => {
    asked = url;
    return Response.json({ codeVerified: true, cloudflareVerified: false, live: false });
  });

  await verifyPendingClaim({ ...CLAIM, handle: "a b" });

  assert.equal(asked, "/api/inbox/verify?handle=a%20b");
});

test("verifyPendingClaim reports a claim that finished while the page was closed as live", async () => {
  stubVerifyEndpoint(() =>
    Response.json({ codeVerified: true, cloudflareVerified: true, live: true })
  );

  assert.deepEqual(await verifyPendingClaim(CLAIM), { kind: "live" });
});

test("verifyPendingClaim reports a claim the server no longer holds as gone", async () => {
  stubVerifyEndpoint(() =>
    Response.json({ error: "Nothing is being claimed here" }, { status: 404 })
  );

  assert.deepEqual(await verifyPendingClaim(CLAIM), { kind: "gone" });
});

/// Discarding the claim here would cost its owner one of the five a wallet gets
/// in an hour, for a fault that is ours and momentary.
test("verifyPendingClaim reports Cloudflare being unreachable as unknown, not as a lost claim", async () => {
  stubVerifyEndpoint(() => Response.json({ error: "Waiting on Cloudflare" }, { status: 503 }));

  assert.deepEqual(await verifyPendingClaim(CLAIM), { kind: "unknown" });
});

test("verifyPendingClaim reports a request that never lands as unknown", async () => {
  globalThis.fetch = (async () => {
    throw new Error("offline");
  }) as typeof fetch;

  assert.deepEqual(await verifyPendingClaim(CLAIM), { kind: "unknown" });
});

test("verifyPendingClaim reports a reply that is not JSON as unknown", async () => {
  stubVerifyEndpoint(() => new Response("<html>gateway</html>", { status: 200 }));

  assert.deepEqual(await verifyPendingClaim(CLAIM), { kind: "unknown" });
});

test("verifyPendingClaim takes the steps the server has confirmed", async () => {
  stubVerifyEndpoint(() =>
    Response.json({ codeVerified: true, cloudflareVerified: false, live: false })
  );

  assert.deepEqual(await verifyPendingClaim(CLAIM), {
    kind: "pending",
    claim: { ...CLAIM, codeVerified: true },
  });
});

/// A step that has been confirmed cannot become unconfirmed. Treating one as
/// undone would put the code field back in front of somebody who has already
/// spent their code, and they only get so many attempts.
test("verifyPendingClaim never un-verifies a step the stored claim had already passed", async () => {
  stubVerifyEndpoint(() =>
    Response.json({ codeVerified: false, cloudflareVerified: false, live: false })
  );

  assert.deepEqual(await verifyPendingClaim({ ...CLAIM, codeVerified: true }), {
    kind: "pending",
    claim: { ...CLAIM, codeVerified: true },
  });
});
