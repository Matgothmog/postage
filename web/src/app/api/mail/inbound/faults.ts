import { required } from "@/lib/env";
import { causeMessage } from "@/lib/errors";

/// Which part of the gateway failed. Not a verdict on the message: every one of
/// these is ours, and the same message would have been judged the same way a
/// minute earlier.
export type FaultStage = "config" | "database" | "chain" | "unexpected";

/// What the caller is told, per stage.
///
/// Fixed strings rather than whatever the dependency said. libsql quotes the
/// database path it could not open, viem quotes the whole RPC URL it called,
/// and a refusing upstream can echo any of it back — useful to whoever operates
/// this, and this is the one that goes out over the wire.
const SAFE_MESSAGE: Record<FaultStage, string> = {
  config: "The gateway is missing a required setting",
  database: "The gateway could not reach its database",
  chain: "The gateway could not read the price floor from the chain",
  unexpected: "The gateway failed for an unexpected reason",
};

/// Every fault answers with the status this route has always given one.
///
/// Unchanged on purpose. The mail worker reads the status to decide a real
/// message's fate: 404 is the only code it takes as an answer rather than as
/// the gateway having fallen over, and every other non-2xx makes it refuse the
/// SMTP session — which Cloudflare emits as a permanent 5.7.1 that Gmail does
/// not hold and retry, so the message is destroyed rather than deferred.
/// Moving a fault onto a different code would move which mail bounces. What was
/// missing was never the code: the body was zero bytes, so the only thing the
/// worker could log was `Gateway returned 500: ` with nothing after the colon.
const FAULT_STATUS = 500;

/// A failure of the gateway itself, carrying the part that failed and a message
/// safe to say out loud.
export class GatewayFault extends Error {
  readonly stage: FaultStage;

  constructor(stage: FaultStage, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "GatewayFault";
    this.stage = stage;
  }
}

/// Runs one step of the route under the name of the thing that can fail in it,
/// so a failure says which without anyone having to read the exception's own
/// words — which are written by a dependency and cannot be relied on to mean
/// the same thing next release.
export async function during<T>(stage: FaultStage, step: () => Promise<T>): Promise<T> {
  try {
    return await step();
  } catch (cause) {
    if (cause instanceof GatewayFault) throw cause;
    throw new GatewayFault(stage, SAFE_MESSAGE[stage], { cause });
  }
}

/// `required()` under the same labelling, and the one fault allowed to name
/// what it was: which variable is unset is the whole diagnosis and the whole
/// fix, and a variable's name is not its value.
export function configured(name: string): string {
  try {
    return required(name);
  } catch (cause) {
    throw new GatewayFault("config", `${name} is not set on the gateway`, { cause });
  }
}

/// The same check for a variable that has to be read before the caller has
/// proved anything, answering without saying which one it was.
///
/// `/api/world/context` states the policy: the credential is checked before any
/// other setting is read, so an anonymous caller learns nothing about how this
/// deployment is configured. This route cannot follow it by ordering alone,
/// because the credential *is* the setting — the webhook secret has to be read
/// to know whether the caller is anyone. So the answer names the class of
/// failure rather than the variable, and the name goes to `console.error` under
/// `cause`, where `faultResponse` logs it and only an operator reads it.
export function configuredWithoutNaming(name: string): string {
  try {
    return required(name);
  } catch (cause) {
    throw new GatewayFault("config", SAFE_MESSAGE.config, { cause });
  }
}

/// The same check, made before the work that needs the variables rather than
/// left to whichever library first reaches for one. `signQuote` hands a missing
/// key to viem, which throws about a malformed private key rather than about a
/// deployment nobody finished configuring.
export function requireConfigured(...names: string[]): void {
  for (const name of names) configured(name);
}

/// Environment variables whose *value* must never reach a log line. Matched on
/// the name rather than kept as a list of variables, so a new variable whose
/// name says what it holds — anything with `SECRET`, `KEY`, `TOKEN`,
/// `PASSWORD` or `CREDENTIAL` in it — is covered the day it is added.
///
/// That is the whole guarantee, and it is narrower than it looks: coverage
/// follows the *name*, so a credential under a name holding none of these words
/// travels intact and covering it means editing this pattern. The three URL
/// names in it are exactly that edit, made three times already: a URL is the
/// other place a credential routinely hides. `DATABASE_URL` carries whatever
/// the driver authenticates with, a keyed `*_RPC_URL` carries its key in the
/// path, and so does a `*_QUERY_URL` — a Graph Studio endpoint is a gateway URL
/// with the API key as a path segment. `APP_URL` and `MAIL_WORKER_URL` are
/// public and stay readable: losing them would cost a diagnosis and protect
/// nothing.
const SECRET_NAME = /SECRET|KEY|TOKEN|PASSWORD|CREDENTIAL|DATABASE_URL|RPC_URL|QUERY_URL/;

/// Shortest value worth hiding. A one- or two-character value is not a secret,
/// and blanking every occurrence of it would shred the message it appears in.
const SHORTEST_SECRET = 8;

/// Whatever a dependency put in an error message, with this process's own
/// secrets taken back out of it. The log is where the real reason has to
/// survive, and it is also the thing that gets shipped to an aggregator.
///
/// The match is on the literal value, so a secret a dependency percent-encoded
/// into a URL before quoting it is a different string and comes through. Both
/// limits — this one and the name matching above — are pinned in
/// `faults.test.ts` rather than left to be discovered.
export function redact(text: string): string {
  let safe = text;
  for (const [name, value] of Object.entries(process.env)) {
    if (!value || value.length < SHORTEST_SECRET) continue;
    if (!SECRET_NAME.test(name)) continue;
    safe = safe.replaceAll(value, `[redacted ${name}]`);
  }
  return safe;
}

/// How far down a chain of causes to read. Deep enough for the wrappers a
/// fetch failure arrives under, short enough that a cycle cannot run away.
const MAX_CAUSE_LINKS = 5;

/// A thrown value's own message, and the messages under it.
///
/// The top of that chain is routinely its least useful link: an unreachable
/// database surfaces as `fetch failed` and nothing else, while the sentence
/// worth having — `connect ECONNREFUSED 10.0.0.1:443` — sits one `cause` down.
function reasonChain(cause: unknown): string {
  const links: string[] = [];
  let link: unknown = cause;
  while (link !== undefined && link !== null && links.length < MAX_CAUSE_LINKS) {
    links.push(causeMessage(link));
    link = link instanceof Error ? link.cause : undefined;
  }
  return links.join(": ");
}

/// The answer to a fault: the status this route has always given one, and a
/// body saying which part of the gateway broke.
///
/// The two halves are deliberately different. The body carries a stable stage
/// and a sentence written here, which is what the worker logs and what tells an
/// operator whether to look at the database, the chain, or the environment. The
/// real words go to `console.error`, where they are worth having and where the
/// caller cannot read them.
export function faultResponse(cause: unknown): Response {
  const fault =
    cause instanceof GatewayFault
      ? cause
      : new GatewayFault("unexpected", SAFE_MESSAGE.unexpected, { cause });

  console.error("inbound mail gateway fault", {
    stage: fault.stage,
    reason: redact(reasonChain(fault.cause ?? fault)),
  });

  return Response.json({ error: fault.message, fault: fault.stage }, { status: FAULT_STATUS });
}
