import assert from "node:assert/strict";
import { createPublicKey, generateKeyPairSync, sign, verify } from "node:crypto";
import type { KeyObject, webcrypto } from "node:crypto";
import { afterEach, beforeEach, test } from "node:test";
import { now } from "./time";

/// Every token here is minted in this process from a key pair generated in this
/// process. A fixture private key checked into the repo would be a credential in
/// the repo whatever it was minted for, and nothing about these assertions needs
/// the same key twice.

const APP_ID = "test-privy-app-id";
process.env.NEXT_PUBLIC_PRIVY_APP_ID = APP_ID;

const SUBJECT = "did:privy:cm2testsubject";
const EMAIL = "someone@example.com";
const WALLET = "0x1111111111111111111111111111111111111111";

type PrivyModule = typeof import("./privy");
type ReadIdentity = PrivyModule["readIdentity"];

/// The `kid` is kept beside the key rather than inside it: the JWKS entry
/// carries one and `createPublicKey` has no use for it, so the two places that
/// want it ask for it separately.
interface SigningKey {
  kid: string;
  jwk: webcrypto.JsonWebKey;
  privateKey: KeyObject;
}

const PUBLISHED = mintKey("published");
const ALSO_PUBLISHED = mintKey("also-published");
const ROTATED = mintKey("rotated");

/// The module reads the clock after a token was minted, so an assertion about
/// one exact second is otherwise a race with the second hand: mint at T, verify
/// at T+1, and "expires this second" has quietly become "expired a second ago".
const FROZEN_MS = Date.UTC(2026, 8, 8, 12, 0, 0);

const realDateNow = Date.now;
const realFetch = globalThis.fetch;

let jwksRequests = 0;
let instances = 0;

beforeEach(() => {
  Date.now = () => FROZEN_MS;
  jwksRequests = 0;
});

afterEach(() => {
  Date.now = realDateNow;
  globalThis.fetch = realFetch;
});

test("a token signed by the key Privy publishes yields the identity its claims name", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.deepEqual(await readIdentity(tokenFrom(PUBLISHED)), {
    userId: SUBJECT,
    email: EMAIL,
    wallets: [WALLET],
  });
});

test("an email and a wallet are lower-cased, so they compare against what is stored", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const shouted = tokenFrom(PUBLISHED, {
    claims: {
      linked_accounts: JSON.stringify([
        { type: "email", address: "Someone@Example.COM" },
        { type: "wallet", address: "0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" },
      ]),
    },
  });

  assert.deepEqual(await readIdentity(shouted), {
    userId: SUBJECT,
    email: EMAIL,
    wallets: ["0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
  });
});

test("a token with no linked accounts yields an identity with neither email nor wallet", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.deepEqual(await readIdentity(tokenFrom(PUBLISHED, { claims: { linked_accounts: undefined } })), {
    userId: SUBJECT,
    email: null,
    wallets: [],
  });
});

/// Privy stringifies the claim, which is why the module parses it, but nothing
/// in the token's own format requires that. A JSON array in the claim has to
/// read the same way or a change at their end silently empties every identity.
test("linked_accounts arriving as an array rather than a string is read the same way", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const nested = tokenFrom(PUBLISHED, {
    claims: {
      linked_accounts: [
        { type: "email", address: EMAIL },
        { type: "wallet", address: WALLET },
      ],
    },
  });

  assert.deepEqual(await readIdentity(nested), { userId: SUBJECT, email: EMAIL, wallets: [WALLET] });
});

test("a linked account that is not a 0x address is left out of the wallets", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const odd = tokenFrom(PUBLISHED, {
    claims: {
      linked_accounts: JSON.stringify([
        { type: "wallet", address: "vitalik.eth" },
        { type: "wallet" },
        { type: "wallet", address: WALLET },
      ]),
    },
  });

  assert.deepEqual((await readIdentity(odd))?.wallets, [WALLET]);
});

/// The one line under test is `header.alg !== ALGORITHM`, and this token is
/// built so that line is the only thing between it and an identity: it is signed
/// with the published key over its own header, so the signature genuinely
/// verifies, and its claims are the claims the accepted token above carries.
/// Both facts are asserted before the rejection is, because a token that merely
/// looks forged would prove only that the parser refuses garbage.
test("a token whose header claims alg none is rejected, though its signature verifies", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const forged = tokenFrom(PUBLISHED, { header: { alg: "none" } });

  assert.ok(
    signatureVerifies(forged, PUBLISHED),
    "precondition: the signature must verify, or the alg check is not what rejects this"
  );
  assert.ok(
    await readIdentity(tokenFrom(PUBLISHED)),
    "precondition: these claims are accepted when the header says ES256"
  );

  assert.equal(await readIdentity(forged), null);
});

/// The same argument for the other half of the confusion attack: a real ES256
/// signature under a header that names a symmetric algorithm.
test("a token whose header claims HS256 is rejected, though it is signed with ES256", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const confused = tokenFrom(PUBLISHED, { header: { alg: "HS256" } });

  assert.ok(
    signatureVerifies(confused, PUBLISHED),
    "precondition: the signature must verify, or the alg check is not what rejects this"
  );

  assert.equal(await readIdentity(confused), null);
});

test("an aud listing this app among others is accepted", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const many = tokenFrom(PUBLISHED, { claims: { aud: ["some-other-app", APP_ID] } });

  assert.equal((await readIdentity(many))?.userId, SUBJECT);
});

test("an aud array naming only other apps is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const elsewhere = tokenFrom(PUBLISHED, { claims: { aud: ["some-other-app", "a-third-app"] } });

  assert.equal(await readIdentity(elsewhere), null);
});

test("an aud naming another app is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { aud: "some-other-app" } })), null);
});

test("an aud that is neither a string nor an array is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { aud: { id: APP_ID } } })), null);
});

test("a token from another issuer is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { iss: "evil.io" } })), null);
});

test("a token that expired a second ago is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { exp: now() - 1 } })), null);
});

test("a token whose exp is the current second is rejected, because expiry is not inclusive", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { exp: now() } })), null);
});

test("a token whose exp is one second away is still live", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal((await readIdentity(tokenFrom(PUBLISHED, { claims: { exp: now() + 1 } })))?.userId, SUBJECT);
});

/// A string expiry is the shape that decides everything if it is compared rather
/// than typed: `"9999999999" > 1789000000` is false in JavaScript, so a lenient
/// read of this claim rejects live tokens, and `<=` on the other side of a
/// refactor would accept dead ones.
test("a token whose exp is a string rather than a number is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { exp: String(now() + 600) } })), null);
});

test("a token carrying no exp at all is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { exp: undefined } })), null);
});

test("a token whose sub is not a string is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED, { claims: { sub: 12345 } })), null);
});

/// Privy rotates, and a token minted before a rotation names a kid the JWKS no
/// longer lists. One published key means there is only one answer to give, and
/// the signature still has to verify against it.
test("a JWKS holding one key signs for a kid it does not name", async () => {
  serveJwks(ROTATED);
  const readIdentity = await freshReadIdentity();
  const stale = tokenFrom(ROTATED, { header: { kid: "a-kid-nobody-publishes" } });

  assert.equal((await readIdentity(stale))?.userId, SUBJECT);
});

test("a header carrying no kid is served by the only key in the JWKS", async () => {
  serveJwks(ROTATED);
  const readIdentity = await freshReadIdentity();

  assert.equal((await readIdentity(tokenFrom(ROTATED, { header: { kid: undefined } })))?.userId, SUBJECT);
});

/// The fallback is a guess, and with two keys published there is nothing to
/// guess from - taking the first would accept a token that names neither.
test("a kid that matches nothing is rejected once the JWKS holds more than one key", async () => {
  serveJwks(PUBLISHED, ALSO_PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const unnamed = tokenFrom(PUBLISHED, { header: { kid: "a-kid-nobody-publishes" } });

  assert.ok(
    signatureVerifies(unnamed, PUBLISHED),
    "precondition: a published key really did sign this, so only the kid lookup can reject it"
  );

  assert.equal(await readIdentity(unnamed), null);
});

/// Naming a kid does not make the key that signed it the key it names. This is
/// the check that stops a JWKS entry anyone can obtain from vouching for a
/// signature made with something else.
test("a token signed by one published key but claiming the kid of another is rejected", async () => {
  serveJwks(PUBLISHED, ALSO_PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const misnamed = tokenFrom(ALSO_PUBLISHED, { header: { kid: PUBLISHED.kid } });

  assert.ok(
    signatureVerifies(misnamed, ALSO_PUBLISHED),
    "precondition: this is a real signature by a published key, just not the one it names"
  );

  assert.equal(await readIdentity(misnamed), null);
});

test("a JWKS that lists no keys at all reads no token", async () => {
  globalThis.fetch = async () => {
    jwksRequests += 1;
    return jsonResponse({});
  };
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(tokenFrom(PUBLISHED)), null);
});

test("a payload swapped in after signing is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const [head, , signature] = tokenFrom(PUBLISHED).split(".");
  const swapped = [head, encode({ ...honestClaims(), sub: "did:privy:someone-else" }), signature].join(".");

  assert.equal(await readIdentity(swapped), null);
});

test("a token with no signature segment is rejected", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const [head, payload] = tokenFrom(PUBLISHED).split(".");

  assert.equal(await readIdentity(`${head}.${payload}`), null);
});

/// A JWT has exactly three segments. `token.split(".")` destructured into
/// three names silently drops anything past the third rather than refusing
/// it, so a genuine token with trailing junk appended used to verify: the
/// first three segments are exactly the ones that were signed.
test("a token with segments trailing the signature is rejected, not truncated to the first three", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const extended = `${tokenFrom(PUBLISHED)}.junk.more`;

  assert.equal(await readIdentity(extended), null);
});

test("a header that is not JSON is rejected rather than thrown out of", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const [, payload, signature] = tokenFrom(PUBLISHED).split(".");
  const garbled = [Buffer.from("{not json").toString("base64url"), payload, signature].join(".");

  assert.equal(await readIdentity(garbled), null);
});

/// The claim is parsed, so it is a second parser reachable from an attacker's
/// half of the token - and it runs after the signature check, which means only
/// Privy can reach it. Either way it must not escape as an exception.
test("linked_accounts that is not parseable JSON is rejected rather than thrown out of", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const garbled = tokenFrom(PUBLISHED, { claims: { linked_accounts: "[{unclosed" } });

  assert.equal(await readIdentity(garbled), null);
});

test("an absent token is no identity, and asks the JWKS nothing", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  assert.equal(await readIdentity(null), null);
  assert.equal(await readIdentity(""), null);
  assert.equal(jwksRequests, 0);
});

/// This no longer holds for every token - one naming a kid absent from the
/// cache now costs a guarded refetch, covered below. What still holds, and
/// is what this pins, is the ordinary case: a kid the cache already lists
/// costs nothing beyond the first fetch, however many tokens name it.
test("the JWKS is fetched once and reused for every token whose kid it already knows", async () => {
  serveJwks(PUBLISHED);
  const readIdentity = await freshReadIdentity();

  await readIdentity(tokenFrom(PUBLISHED));
  await readIdentity(tokenFrom(PUBLISHED));

  assert.equal(jwksRequests, 1);
});

/// The rescue for the case a fixed cache can't: Privy rotates, a token minted
/// after the rotation names a kid the cache built before it has never heard
/// of, and unlike the single-key fallback above, this has to actually go
/// look rather than guess.
test("a kid absent from the cached JWKS triggers one refetch, and the rotated key found there is accepted", async () => {
  let servedKeys = [PUBLISHED];
  globalThis.fetch = async () => {
    jwksRequests += 1;
    return jsonResponse({ keys: servedKeys.map((key) => ({ ...key.jwk, kid: key.kid })) });
  };
  const readIdentity = await freshReadIdentity();

  assert.equal((await readIdentity(tokenFrom(PUBLISHED)))?.userId, SUBJECT);
  assert.equal(jwksRequests, 1, "precondition: the cache is primed with the pre-rotation key");

  // Privy rotates: the endpoint now serves a different key, and a token
  // signed with it names a kid the primed cache has never seen.
  servedKeys = [ROTATED];
  const afterRotation = tokenFrom(ROTATED);

  assert.equal((await readIdentity(afterRotation))?.userId, SUBJECT);
  assert.equal(jwksRequests, 2, "an unknown kid must cost exactly one refetch, not zero and not more");
});

/// The guard on that refetch: with more than one key cached, an unknown kid
/// gets no fallback to fall back on, so nothing but the guard stands between
/// a stream of tokens naming a kid nobody ever published and a stream of
/// requests to Privy for each one.
test("a kid that matches nothing does not cause unbounded refetching", async () => {
  serveJwks(PUBLISHED, ALSO_PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const bogus = tokenFrom(PUBLISHED, { header: { kid: "never-published" } });

  assert.equal(await readIdentity(bogus), null);
  assert.equal(jwksRequests, 2, "the first read gets one guarded refetch, on top of the initial fetch");

  await readIdentity(bogus);
  await readIdentity(bogus);

  assert.equal(jwksRequests, 2, "repeats within the guard window must not refetch again");
});

/// The guard is a cooldown, not a permanent lockout: once it elapses, a
/// still-unknown kid is allowed to try again, because a rotation could
/// genuinely have landed in that window.
test("the refetch guard releases once its window has passed", async () => {
  serveJwks(PUBLISHED, ALSO_PUBLISHED);
  const readIdentity = await freshReadIdentity();
  const bogus = tokenFrom(PUBLISHED, { header: { kid: "never-published" } });

  await readIdentity(bogus);
  assert.equal(jwksRequests, 2);

  Date.now = () => FROZEN_MS + 61_000;
  await readIdentity(bogus);

  assert.equal(jwksRequests, 3, "the window has passed, so this read gets its own guarded refetch");
});

/// The failure is what must not be cached. Remembered, one unreachable minute
/// rejects every token for the life of the process, and every signed-in user is
/// pushed onto the long signup path until something replaces it.
test("a JWKS that failed to load is not remembered, so the next token still reads", async () => {
  globalThis.fetch = async () => {
    jwksRequests += 1;
    return new Response("gateway is unhappy", { status: 503 });
  };
  const readIdentity = await freshReadIdentity();
  assert.equal(await readIdentity(tokenFrom(PUBLISHED)), null);

  serveJwks(PUBLISHED);

  assert.equal((await readIdentity(tokenFrom(PUBLISHED)))?.userId, SUBJECT);
  assert.equal(jwksRequests, 2, "the second read has to have asked again rather than reused the failure");
});

/// The other half of not remembering a failure is not asking again for every
/// caller that arrives while it lasts. A JWKS that is down answers as fast as
/// it can, so an unguarded retry per inbound request turns our traffic into
/// its traffic - the shape that holds a recovering dependency down - and bills
/// us for the egress while it does it.
test("a JWKS that keeps failing is not asked again once per read", async () => {
  globalThis.fetch = async () => {
    jwksRequests += 1;
    return new Response("gateway is unhappy", { status: 503 });
  };
  const readIdentity = await freshReadIdentity();

  for (let read = 0; read < 25; read += 1) {
    assert.equal(await readIdentity(tokenFrom(PUBLISHED)), null);
  }

  assert.equal(
    jwksRequests,
    2,
    "one attempt and one retry, and then the cooldown answers the other 23 reads"
  );
});

/// What the cooldown costs the caller, stated on purpose: no identity, which
/// is what an unreachable JWKS already cost them, and the longer signup path.
/// What it must not cost is the recovery - a JWKS that comes back has to be
/// found within the window rather than when the process is next replaced.
test("a JWKS that recovers is read again as soon as the cooldown lapses", async () => {
  globalThis.fetch = async () => {
    jwksRequests += 1;
    return new Response("gateway is unhappy", { status: 503 });
  };
  const readIdentity = await freshReadIdentity();
  for (let read = 0; read < 3; read += 1) await readIdentity(tokenFrom(PUBLISHED));

  assert.equal(jwksRequests, 2, "precondition: the cooldown is in force");

  // Healthy again, but still inside the window nothing is allowed to ask in.
  serveJwks(PUBLISHED);

  assert.equal(await readIdentity(tokenFrom(PUBLISHED)), null, "no identity while it holds");
  assert.equal(jwksRequests, 2, "and no request either");

  Date.now = () => FROZEN_MS + 61_000;

  assert.equal((await readIdentity(tokenFrom(PUBLISHED)))?.userId, SUBJECT);
  assert.equal(jwksRequests, 3, "the window has passed, so one read goes and finds it recovered");
});

/// A `readIdentity` with a JWKS cache of its own.
///
/// The module remembers the first key set it successfully fetches for the life
/// of the process, so tests sharing one instance would all be asserting against
/// whichever JWKS happened to load first - and the single-key fallback could
/// never be reached at all. The query string is what makes the loader hand back
/// a new copy, module-level cache included.
async function freshReadIdentity(): Promise<ReadIdentity> {
  instances += 1;
  const specifier: string = `./privy.ts?instance=${instances}`;
  const loaded = (await import(specifier)) as PrivyModule;
  return loaded.readIdentity;
}

/// Privy's JWKS sits behind a hardcoded https URL, so `fetch` is the only place
/// to stand between the module and the network.
function serveJwks(...keys: SigningKey[]): void {
  globalThis.fetch = async () => {
    jwksRequests += 1;
    return jsonResponse({ keys: keys.map((key) => ({ ...key.jwk, kid: key.kid })) });
  };
}

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" },
  });
}

function mintKey(kid: string): SigningKey {
  const { privateKey, publicKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  return { kid, jwk: publicKey.export({ format: "jwk" }), privateKey };
}

/// The claims Privy actually sends, in the shape it sends them.
function honestClaims(): Record<string, unknown> {
  return {
    iss: "privy.io",
    aud: APP_ID,
    sub: SUBJECT,
    exp: now() + 600,
    linked_accounts: JSON.stringify([
      { type: "email", address: EMAIL },
      { type: "wallet", address: WALLET },
    ]),
  };
}

interface TokenShape {
  header?: Record<string, unknown>;
  claims?: Record<string, unknown>;
}

/// A token the module accepts, unless `shape` changes the one thing under test.
/// Every negative case below is this token with a single field moved, which is
/// what makes the rejection attributable to that field.
function tokenFrom(key: SigningKey, shape: TokenShape = {}): string {
  const head = encode({ alg: "ES256", typ: "JWT", kid: key.kid, ...shape.header });
  const payload = encode({ ...honestClaims(), ...shape.claims });
  const signature = sign("sha256", Buffer.from(`${head}.${payload}`), {
    key: key.privateKey,
    dsaEncoding: "ieee-p1363",
  });
  return `${head}.${payload}.${signature.toString("base64url")}`;
}

function encode(value: unknown): string {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}

/// Independent proof that a rejected token carries a signature that is good.
/// Without it a negative test says only that the module refuses something, not
/// which of its checks refused - and the answer "the signature did" would make
/// the test worthless as coverage of anything else.
function signatureVerifies(token: string, key: SigningKey): boolean {
  const [head, payload, signature] = token.split(".");
  return verify(
    "sha256",
    Buffer.from(`${head}.${payload}`),
    { key: createPublicKey({ key: key.jwk, format: "jwk" }), dsaEncoding: "ieee-p1363" },
    Buffer.from(signature, "base64url")
  );
}
