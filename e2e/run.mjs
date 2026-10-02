// Local end-to-end run: the real Axum router over a real libsql file, the
// real browser bundle (crates/web/dist) in headless Firefox, and loopback
// stand-ins for every outside service. Nothing leaves the machine.
//
//   node e2e/run.mjs [--shots <dir>] [--rebuild-web] [--keep]
//
// Needs: cargo, trunk (with npm for the bridge), geckodriver + Firefox, and
// Node 23+ (node:sqlite). See e2e/README.md.
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { sleep, startBrowser } from "./webdriver.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const option = (name, fallback) => {
  const at = args.indexOf(name);
  return at === -1 ? fallback : args[at + 1];
};
const shots = resolve(option("--shots", join(tmpdir(), "postage-e2e-shots")));
mkdirSync(shots, { recursive: true });

// ---------------------------------------------------------------- building

function build() {
  const dist = join(repo, "crates/web/dist/index.html");
  if (flag("--rebuild-web") || !existsSync(dist)) {
    if (!existsSync(join(repo, "crates/web/js/node_modules"))) {
      run("npm", ["ci", "--prefix", "crates/web/js"], repo);
    }
    run("trunk", ["build"], join(repo, "crates/web"), {
      NEXT_PUBLIC_PRIVY_APP_ID: "dummy-privy-app-id",
      NEXT_PUBLIC_WORLD_APP_ID: "app_dummy",
      NEXT_PUBLIC_WORLD_ENVIRONMENT: "sandbox",
    });
  }
  const built = spawnSync(
    "cargo",
    ["build", "-p", "postage-server", "--example", "e2e_server", "--message-format=json-render-diagnostics"],
    { cwd: repo, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"], maxBuffer: 1 << 28 }
  );
  if (built.status !== 0) throw new Error("cargo build of the e2e server failed");
  const artifact = built.stdout
    .split("\n")
    .filter((line) => line.startsWith("{"))
    .map((line) => JSON.parse(line))
    .find((message) => message.reason === "compiler-artifact" && message.target?.name === "e2e_server" && message.executable);
  if (!artifact) throw new Error("cargo did not report the e2e_server executable");
  return artifact.executable;
}

function run(command, commandArgs, cwd, env = {}) {
  const result = spawnSync(command, commandArgs, { cwd, stdio: "inherit", env: { ...process.env, ...env } });
  if (result.status !== 0) throw new Error(`${command} ${commandArgs.join(" ")} failed`);
}

// ------------------------------------------------------------------ server

async function startServer(executable, dbPath) {
  const child = spawn(executable, ["--db", dbPath, "--dist", join(repo, "crates/web/dist"), "--repo", repo], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  const stderr = [];
  child.stderr.on("data", (chunk) => stderr.push(...chunk.toString().split("\n").filter(Boolean)));
  const ready = await new Promise((resolveReady, reject) => {
    let buffered = "";
    child.once("exit", (code) => reject(new Error(`server exited early (${code}): ${stderr.join("\n")}`)));
    child.stdout.on("data", (chunk) => {
      buffered += chunk.toString();
      const line = buffered.split("\n").find((candidate) => candidate.startsWith("E2E_READY "));
      if (line) resolveReady(JSON.parse(line.slice("E2E_READY ".length)));
    });
  });
  const stubs = async (service) =>
    (await (await fetch(`${ready.stubs}/admin/requests${service ? `?service=${service}` : ""}`)).json());
  const admin = async (path) => (await fetch(`${ready.stubs}/admin${path}`, { method: "POST" })).json();
  return { child, ready, stderr, stubs, admin, stop: () => child.kill() };
}

/** Rows from the server's database, read through a fresh read-only handle. */
function rows(dbPath, sql, ...params) {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    return db.prepare(sql).all(...params);
  } finally {
    db.close();
  }
}

// ----------------------------------------------------------------- results

const results = [];
let currentJourney = "";
function journey(name) {
  currentJourney = name;
  console.log(`\n== ${name}`);
}
function check(what, ok, evidence = "") {
  results.push({ journey: currentJourney, what, ok: Boolean(ok), evidence: String(evidence) });
  console.log(`  ${ok ? "PASS" : "FAIL"}  ${what}${evidence !== "" ? `  [${evidence}]` : ""}`);
}
function note(text) {
  console.log(`  note  ${text}`);
}
/** The calldata of a recorded eth_call (alloy sends it as `input`, viem as `data`). */
const callData = (request) => request.body?.params?.[0]?.input ?? request.body?.params?.[0]?.data ?? "";
const short = (value) => (JSON.stringify(value) ?? "none").slice(0, 220);

// -------------------------------------------------------------- the script

const executable = build();
const workdir = mkdtempSync(join(tmpdir(), "postage-e2e-"));
const dbPath = join(workdir, "e2e.db");
const server = await startServer(executable, dbPath);
const { origin } = server.ready;
const { owner, sender } = server.ready.config.personas;
const inbound = (message, secret = server.ready.webhookSecret) =>
  fetch(`${origin}/api/mail/inbound`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "x-postage-secret": secret },
    // DMARC passed for a From: header on the envelope's own domain, as the worker reports it.
    body: JSON.stringify({ dmarc: "pass", header_from: message.from, ...message }),
  });
const shot = (browser, name) => browser.screenshot(join(shots, `${name}.png`));
const HANDLE = "alice";
const DESTINATION = "reader@example.net";

let browser;
try {
  browser = await startBrowser();
  await claimJourney();
  const held = await inboundJourney();
  await humanLaneJourney(held);
  await paidLaneJourney();
  await deliverLaneJourney();
  await networkJourney();
  await concurrencyCheck();
  await fieldOrderJourney();
} catch (error) {
  check(`journey aborted: ${error.message.split("\n")[0]}`, false, error.stack);
} finally {
  await browser?.close();
  server.stop();
}

// 1 ------------------------------------------------------------------------
async function claimJourney() {
  journey("1. Claim: sign in, pick a handle, mailed code, Cloudflare, live inbox");
  await browser.goto(`${origin}/`);
  await browser.clearStorage();
  await browser.setStorage("e2e.persona", "owner");
  await browser.goto(`${origin}/`);
  await browser.waitForText("Make spam pay.");
  await shot(browser, "01-landing");

  await browser.fill("input[placeholder=you]", HANDLE);
  await browser.clickButton("Claim it");
  const form = await browser.waitForText("Pick your address.");
  check("signing in (mock Privy) lands on the address form, signed in", form.includes("Sign out"));
  await shot(browser, "02-claim-form");

  await browser.fill("#destination", DESTINATION);
  await browser.clickButton("Claim it");
  const coded = await browser.waitForText(`We emailed a code to ${DESTINATION}`);
  check("a different destination asks for a mailed code", coded.includes("Code from your email"));
  await shot(browser, "03-code-prompt");

  const claim = rows(dbPath, "SELECT * FROM inbox_claims WHERE handle = ?", HANDLE)[0];
  check(
    "DB: inbox_claims holds the pending claim for the signed-in wallet",
    claim && claim.destination === DESTINATION && claim.wallet === owner.wallet && claim.code_verified_at === null,
    claim && `wallet=${claim.wallet} destination=${claim.destination} code_verified_at=${claim.code_verified_at}`
  );
  const sends = rows(dbPath, "SELECT destination, wallet FROM claim_sends");
  check("DB: the code send was counted against the throttle", sends.length === 1 && sends[0].destination === DESTINATION, short(sends));

  const mails = await server.stubs("resend");
  const codeMail = mails.find((mail) => mail.body.to === DESTINATION);
  const code = /(\d{6})/.exec(codeMail?.body.subject ?? "")?.[1];
  check("Resend stub captured the verification code mail", Boolean(code), codeMail && `to=${codeMail.body.to} subject="${codeMail.body.subject}"`);

  const wrong = code === "000000" ? "111111" : "000000";
  await browser.fill("input[inputmode=numeric]", wrong);
  const refused = await browser.waitForText(/tries left/);
  check("a wrong code is refused with the attempts left", /That code is wrong\. 4 tries left/.test(refused), refused.match(/That code is wrong[^\n]*/)?.[0]);
  await shot(browser, "04-wrong-code");
  // After a refusal the sixth digit no longer sends by itself; the Confirm
  // button that appears is the way on.
  await browser.fill("input[inputmode=numeric]", code);
  await browser.clickButton("Confirm");
  await browser.waitForText(`Cloudflare emailed ${DESTINATION}`);
  const verified = rows(dbPath, "SELECT * FROM inbox_claims WHERE handle = ?", HANDLE)[0];
  check("DB: the right code is recorded and the Cloudflare address attached", verified?.code_verified_at && verified.cf_address_id === "addr-1", verified && `code_verified_at=${verified.code_verified_at} cf_address_id=${verified.cf_address_id} attempts=${verified.attempts}`);
  const registered = (await server.stubs("cloudflare")).filter((request) => request.method === "POST");
  check("Cloudflare stub was asked to register the destination", registered.length === 1 && registered[0].body.email === DESTINATION, short(registered.map((request) => request.body)));
  await shot(browser, "05-waiting-on-cloudflare");

  await server.admin("/cloudflare/confirm");
  const live = await browser.waitForText("LIVE", { timeout: 30000 });
  check("once Cloudflare verifies, the strip goes live and shows the address and wallet", live.includes(`${HANDLE}@usepostage.com`) && live.includes("Paid into 0x7099...79C8"));
  const inbox = rows(dbPath, "SELECT * FROM inboxes WHERE handle = ?", HANDLE)[0];
  check("DB: inboxes row created with destination and owner wallet", inbox && inbox.destination === DESTINATION && inbox.wallet === owner.wallet, short(inbox));
  check("DB: the pending claim row is gone", rows(dbPath, "SELECT 1 FROM inbox_claims WHERE handle = ?", HANDLE).length === 0);
  const reads = (await server.stubs("rpc")).filter((request) => request.body?.method === "eth_call");
  check("the page read the inbox's standing from the local node, not the internet", reads.some((request) => request.path === "/node"), `${reads.length} eth_call(s)`);
  await shot(browser, "06-inbox-live");

  await browser.goto(`${origin}/`);
  const again = await browser.waitForText("LIVE");
  check("reloading keeps the inbox (GET /api/inbox with the identity token)", again.includes(`${HANDLE}@usepostage.com`));
}

// 2 ------------------------------------------------------------------------
async function inboundJourney() {
  journey("2. Inbound mail from an unknown sender is held");
  const denied = await inbound({ from: "x@y.test", to: `${HANDLE}@usepostage.com` }, "wrong-secret");
  check("a caller without the webhook secret gets 401", denied.status === 401, denied.status);
  const unknown = await inbound({ from: "x@y.test", to: "nobody@usepostage.com", subject: "hi", body: "hi" });
  check("an unknown inbox gets 404 with a reject verdict", unknown.status === 404, `${unknown.status} ${short(await unknown.json())}`);

  const response = await inbound({ from: "anna@acme.test", to: `${HANDLE}@usepostage.com`, subject: "Quick question", body: "Hi, are you free Thursday?" });
  const verdict = await response.json();
  check("POST /api/mail/inbound answers 200 with a hold verdict", response.status === 200 && verdict.action === "hold", `status=${response.status} action=${verdict.action} reason=${verdict.reason}`);
  check("the verdict carries token, held_until and a challenge_url on this origin", verdict.challenge_url === `${origin}/c/${verdict.token}` && verdict.held_until > 0, verdict.challenge_url);
  check("the notice mail for the sender is present (authenticated sender)", Boolean(verdict.notice?.subject), verdict.notice?.subject);
  const challenge = rows(dbPath, "SELECT * FROM challenges WHERE token = ?", verdict.token)[0];
  check("DB: challenges row recorded for the sender, unresolved", challenge && challenge.sender === "anna@acme.test" && challenge.handle === HANDLE && challenge.resolved_at === null, challenge && `tier=${challenge.tier} amount=${challenge.amount} held_until=${challenge.held_until}`);
  const model = await server.stubs("anthropic");
  check("the Anthropic stub classified the message", model.length >= 1, `${model.length} request(s)`);
  const floorReads = (await server.stubs("rpc")).filter((request) => request.body?.method === "eth_call" && callData(request).startsWith("0x552c804e"));
  check("the price came from effectiveFloor on the local node", floorReads.length >= 1, `${floorReads.length} read(s); price=${verdict.price}`);
  return verdict;
}

// 3 ------------------------------------------------------------------------
async function humanLaneJourney(held) {
  journey("3. Human lane: open the challenge, \"I'm human\" (mock), cleared and released");
  await browser.goto(`${origin}/`);
  await browser.setStorage("e2e.persona", "sender");
  await browser.setStorage("e2e.signedIn", "0");
  await browser.goto(held.challenge_url);
  const opened = await browser.waitForText("I'm human — free");
  check("the challenge page names the inbox and offers both lanes", opened.includes(`${HANDLE}@usepostage.com`) && /pay \$0\.05/.test(opened), opened.split("\n").slice(0, 8).join(" | "));
  await shot(browser, "07-challenge-open");

  await browser.clickButton("I'm human — free");
  const sent = await browser.waitForText("Sent.");
  check("the page confirms the message went through", sent.includes("Already in their inbox."));
  await shot(browser, "08-human-cleared");

  const challenge = rows(dbPath, "SELECT * FROM challenges WHERE token = ?", held.token)[0];
  check("DB: challenge resolved on the human lane and delivered", challenge.resolved_at && challenge.settled_by === "human" && challenge.delivered_at, `settled_by=${challenge.settled_by} resolved_at=${challenge.resolved_at} delivered_at=${challenge.delivered_at}`);
  const pass = rows(dbPath, "SELECT * FROM passes WHERE handle = ? AND sender = ?", HANDLE, "anna@acme.test")[0];
  check("DB: the sender was granted a timed pass", pass && pass.reason === "human" && pass.uses_left === null, short(pass));
  check("DB: a nullifier was bound to the sender", rows(dbPath, "SELECT * FROM nullifiers WHERE sender = ?", "anna@acme.test").length === 1);
  const releases = (await server.stubs("worker")).filter((request) => request.path === "/release");
  check("the mail worker stub received the release for this token and destination", releases.length === 1 && releases[0].body.token === held.token && releases[0].body.to === DESTINATION, short(releases.map((request) => request.body)));
  const rpcMethods = (await server.stubs("rpc")).map((request) => request.body?.method);
  check("the attestation went out through the node: sendRawTransaction + receipt", rpcMethods.includes("eth_sendRawTransaction") && rpcMethods.includes("eth_getTransactionReceipt"));
  check("World's verify endpoint was not called in mock identity mode", (await server.stubs("world")).length === 0);

  const reopened = await fetch(`${origin}/api/challenge/${held.token}`).then((response) => response.json());
  check("GET /api/challenge/{token} now reads resolved", reopened.state === "resolved", short(reopened));
  await browser.goto(held.challenge_url);
  const done = await browser.waitForText("dealt with");
  check("reopening the link says it is dealt with", Boolean(done));
  await shot(browser, "09-challenge-resolved");
}

// 4 ------------------------------------------------------------------------
async function paidLaneJourney() {
  journey("4. Paid lane: pay (mock wallet), the node reports it paid, resolve, delivered");
  const response = await inbound({ from: "bob@shop.test", to: `${HANDLE}@usepostage.com`, subject: "[e2e-tier:commercial] 50% off everything", body: "Buy now at https://shop.test/sale" });
  const verdict = await response.json();
  check("the second inbound is held as commercial", verdict.action === "hold" && verdict.reason === "commercial", `reason=${verdict.reason} price=${verdict.price}`);

  await browser.goto(`${origin}/`);
  await browser.setStorage("e2e.persona", "sender");
  await browser.setStorage("e2e.signedIn", "1");
  await browser.goto(`${verdict.challenge_url}?as=bot`);
  await browser.waitForText("Pay $0.05");
  await shot(browser, "10-pay-lane");
  await browser.clickButton("Pay $0.05");
  const sent = await browser.waitForText("Sent.", { timeout: 30000 });
  check("after paying, the page confirms delivery", sent.includes("Already in their inbox."));
  await shot(browser, "11-paid-delivered");

  const calls = await browser.run("return window.__e2eCalls();");
  const tx = calls.find((call) => call.fn === "sendTransaction");
  check("the mock wallet sent payToSend to the escrow with the quoted value", tx && tx.to.toLowerCase() === "0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7" && tx.value === verdict.price && tx.data.startsWith("0xb6ba947b"), tx && `to=${tx.to} value=${tx.value} selector=${tx.data.slice(0, 10)} chainId=${tx.chainId}`);
  const state = await (await fetch(`${server.ready.stubs}/admin/state`)).json();
  check("the local node recorded the payer against the quote's message id", state.payments.some((payment) => payment.messageId === verdict.quote.messageId && payment.payer.toLowerCase() === sender.wallet), short(state.payments));
  const settlementReads = (await server.stubs("rpc")).filter((request) => callData(request).startsWith("0xf1d3d381"));
  check("the server read settlementOf from the node while resolving", settlementReads.length >= 1);
  const challenge = rows(dbPath, "SELECT * FROM challenges WHERE token = ?", verdict.token)[0];
  check("DB: challenge settled by payment and delivered", challenge.settled_by === "paid" && challenge.delivered_at, `settled_by=${challenge.settled_by} delivered_at=${challenge.delivered_at}`);
  const link = rows(dbPath, "SELECT * FROM sender_wallets WHERE sender = ?", "bob@shop.test")[0];
  check("DB: the payer's wallet is linked to the sender", link && link.wallet === sender.wallet, short(link));
  const pass = rows(dbPath, "SELECT * FROM passes WHERE handle = ? AND sender = ?", HANDLE, "bob@shop.test")[0];
  note(`passes row for the payer after the paid lane: ${short(pass)}`);
  const releases = (await server.stubs("worker")).filter((request) => request.path === "/release");
  check("the mail worker stub received a second release", releases.length === 2 && releases[1].body.token === verdict.token, `${releases.length} release(s)`);
}

// 5 ------------------------------------------------------------------------
async function deliverLaneJourney() {
  journey("5. Deliver lane: the hold expired, paste the message, POST /api/challenge/deliver");
  const response = await inbound({ from: "carol@news.test", to: `${HANDLE}@usepostage.com`, subject: "Lunch?", body: "Fancy lunch on Friday?" });
  const verdict = await response.json();
  check("a third sender is held", verdict.action === "hold", `reason=${verdict.reason}`);
  const offset = await server.admin("/clock/advance?seconds=90000");
  note(`server clock moved forward ${offset.offsetSeconds}s, past the 24h hold`);

  await browser.goto(`${origin}/`);
  await browser.setStorage("e2e.persona", "sender");
  await browser.setStorage("e2e.signedIn", "0");
  await browser.goto(`${verdict.challenge_url}?as=human`);
  await browser.waitForText("I'm human — free");
  await browser.clickButton("I'm human — free");
  const expired = await browser.waitForText("The hold expired. Paste it again.");
  check("proving personhood after the hold ran out offers the paste form", expired.includes("Cleared."));
  await shot(browser, "12-hold-expired-paste-form");
  const releases = (await server.stubs("worker")).filter((request) => request.path === "/release");
  check("the worker was not asked to release an expired hold", releases.length === 2, `${releases.length} release(s) in total`);

  await browser.fill("input[placeholder=Subject]", "Lunch?");
  await browser.fill("textarea", "Fancy lunch on Friday? The new place on the corner.");
  await browser.clickButton("Send");
  const delivered = await browser.waitForText("Replies come straight to you.");
  check("POST /api/challenge/deliver succeeds and the page says Sent.", delivered.includes("Sent."));
  await shot(browser, "13-pasted-sent");

  const relayed = (await server.stubs("resend")).filter((mail) => mail.body.subject === "Lunch?");
  const mail = relayed[0]?.body;
  check("the Resend stub received the relay to the inbox's destination", relayed.length === 1 && mail.to === DESTINATION && mail.text.includes("Fancy lunch on Friday"), mail && `to=${mail.to} reply_to=${mail.reply_to} from="${mail.from}"`);
  check("it goes out under Postage's name with the sender in Reply-To", mail?.reply_to === "carol@news.test" && mail.from.includes("hello@usepostage.com"));
  const pass = rows(dbPath, "SELECT * FROM passes WHERE handle = ? AND sender = ?", HANDLE, "carol@news.test")[0];
  check("DB: the human pass is still live (spend is on a timed window)", pass && pass.reason === "human", short(pass));
}

// 6 ------------------------------------------------------------------------
async function networkJourney() {
  journey("6. /network renders from the Graph stub");
  await browser.goto(`${origin}/network`);
  const text = await browser.waitForText(/Paid out/i, { timeout: 20000 });
  check("the ledger page shows the stubbed totals and rows", text.includes("0x3c44") || text.includes("0x70997970"), text.split("\n").filter(Boolean).slice(0, 14).join(" | "));
  await shot(browser, "14-network");
  const queries = (await server.stubs("graph")).filter((request) => String(request.body?.query).includes("query Overview"));
  check("the Graph stub received the Overview query", queries.length >= 1, `${queries.length} query(ies)`);
  const view = await fetch(`${origin}/api/network`).then((response) => response.json());
  check("GET /api/network returns the merged feed with payment and verification entries", view.feed.some((item) => item.kind === "payment") && view.feed.some((item) => item.kind === "verification"), `feed=${view.feed.map((item) => item.kind).join(",")}`);
}

// concurrency --------------------------------------------------------------
async function concurrencyCheck() {
  journey("Concurrency: 20 parallel inbound POSTs, then 20 parallel resolve polls");
  const stderrBefore = server.stderr.length;
  const burst = await Promise.all(
    Array.from({ length: 20 }, (_, index) =>
      inbound({ from: `burst${String(index).padStart(2, "0")}@burst.test`, to: `${HANDLE}@usepostage.com`, subject: `Burst ${index}`, body: `Message number ${index}` }).then(async (response) => ({ status: response.status, body: await response.json() }))
    )
  );
  const statuses = new Set(burst.map((answer) => answer.status));
  check("20 parallel inbound POSTs all answered 200", statuses.size === 1 && statuses.has(200), [...statuses].join(","));
  check("every one was held with its own token", new Set(burst.map((answer) => answer.body.token)).size === 20 && burst.every((answer) => answer.body.action === "hold"));
  const stored = rows(dbPath, "SELECT COUNT(*) AS n FROM challenges WHERE sender LIKE 'burst%'")[0].n;
  check("DB: 20 challenge rows were written", stored === 20, stored);

  const resolve = (token) =>
    fetch(`${origin}/api/challenge/resolve`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ token }) }).then(async (response) => ({ status: response.status, body: await response.json() }));
  const distinct = await Promise.all(burst.map((answer) => resolve(answer.body.token)));
  check("20 parallel resolve polls (distinct tokens) all answered 200 pending", distinct.every((answer) => answer.status === 200 && answer.body.status === "pending"), [...new Set(distinct.map((answer) => `${answer.status}:${answer.body.status}`))].join(","));
  const sameToken = await Promise.all(Array.from({ length: 20 }, () => resolve(burst[0].body.token)));
  check("20 parallel resolve polls on one token all answered 200 pending", sameToken.every((answer) => answer.status === 200 && answer.body.status === "pending"));

  const mixed = await Promise.all([
    ...Array.from({ length: 10 }, (_, index) => inbound({ from: `mixed${index}@burst.test`, to: `${HANDLE}@usepostage.com`, subject: `Mixed ${index}`, body: "x" }).then((response) => response.status)),
    ...burst.slice(0, 10).map((answer) => resolve(answer.body.token).then((reply) => reply.status)),
    ...burst.slice(0, 10).map((answer) => fetch(`${origin}/api/challenge/${answer.body.token}`).then((response) => response.status)),
  ]);
  check("a mixed round of writes and reads (30 requests) had no non-200", mixed.every((status) => status === 200), [...new Set(mixed)].join(","));

  const fresh = server.stderr.slice(stderrBefore);
  const bad = fresh.filter((line) => /locked|busy|panick|unhandled route error/i.test(line));
  check('server stderr has no "database is locked", panics or unhandled route errors', bad.length === 0, bad.length ? bad.slice(0, 3).join(" || ") : `${fresh.length} stderr line(s) during the burst`);
}

// 7 ------------------------------------------------------------------------
async function fieldOrderJourney() {
  journey("7. GET /api/inbox: legacy-migrated DB (ALTER TABLE adds wallet) vs fresh schema");
  const legacyPath = join(workdir, "legacy.db");
  const legacy = new DatabaseSync(legacyPath);
  legacy.exec("CREATE TABLE inboxes (handle TEXT PRIMARY KEY, destination TEXT NOT NULL, created_at INTEGER NOT NULL)");
  legacy.prepare("INSERT INTO inboxes (handle, destination, created_at) VALUES (?, ?, ?)").run(HANDLE, DESTINATION, 1760000000);
  legacy.close();
  const second = await startServer(executable, legacyPath);
  try {
    // Opening the database runs the bootstrap; any request does it.
    await fetch(`${second.ready.origin}/api/health`);
    await fetch(`${second.ready.origin}/api/inbox/verify?handle=nobody`);
    const columns = (db) => rows(db, "SELECT name FROM pragma_table_info('inboxes') ORDER BY cid").map((column) => column.name);
    const starKeys = (db) => Object.keys(rows(db, "SELECT * FROM inboxes LIMIT 1")[0]);
    const legacyCols = columns(legacyPath);
    const freshCols = columns(dbPath);
    check("the legacy table gained wallet by ALTER TABLE (appended last)", legacyCols.join(",") === "handle,destination,created_at,wallet", legacyCols.join(","));
    check("the fresh schema has wallet before created_at", freshCols.join(",") === "handle,destination,wallet,created_at", freshCols.join(","));
    note(`SELECT * column order, legacy: ${starKeys(legacyPath).join(",")}`);
    note(`SELECT * column order, fresh:  ${starKeys(dbPath).join(",")}`);
    check("SELECT * column order differs between the two databases", starKeys(legacyPath).join(",") !== starKeys(dbPath).join(","));

    const legacyDb = new DatabaseSync(legacyPath);
    legacyDb.prepare("UPDATE inboxes SET wallet = ? WHERE handle = ?").run(owner.wallet, HANDLE);
    legacyDb.close();
    const fetchInbox = async (base, persona) => {
      const response = await fetch(`${base}/api/inbox`, { headers: { "privy-id-token": persona.identityToken } });
      return { status: response.status, text: await response.text() };
    };
    const fromLegacy = await fetchInbox(second.ready.origin, second.ready.config.personas.owner);
    const fromFresh = await fetchInbox(origin, owner);
    note(`legacy response: ${fromLegacy.text}`);
    note(`fresh response:  ${fromFresh.text}`);
    const keyOrder = (text) => Object.keys(JSON.parse(text).inbox).join(",");
    check("both servers answer 200 with an inbox", fromLegacy.status === 200 && fromFresh.status === 200 && JSON.parse(fromLegacy.text).inbox && JSON.parse(fromFresh.text).inbox);
    check("the response JSON key order is identical (handle, destination, wallet, created_at)", keyOrder(fromLegacy.text) === keyOrder(fromFresh.text) && keyOrder(fromFresh.text) === "handle,destination,wallet,created_at", `legacy=${keyOrder(fromLegacy.text)} fresh=${keyOrder(fromFresh.text)}`);
  } finally {
    second.stop();
  }
}

// ------------------------------------------------------------------ report

await sleep(100);
const failed = results.filter((result) => !result.ok);
writeFileSync(join(shots, "results.json"), JSON.stringify(results, null, 2));
console.log(`\n${results.length - failed.length}/${results.length} checks passed. Screenshots and results.json: ${shots}`);
if (server.stderr.length > 0) console.log(`server stderr (${server.stderr.length} lines), first 5:\n  ${server.stderr.slice(0, 5).join("\n  ")}`);
if (!flag("--keep")) rmSync(workdir, { recursive: true, force: true });
process.exit(failed.length === 0 ? 0 : 1);
