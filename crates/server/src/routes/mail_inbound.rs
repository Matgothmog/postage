//! `POST /api/mail/inbound` (`web/src/app/api/mail/inbound/route.ts`): called
//! by the mail worker for every inbound message.
//!
//! Every stranger is held. The classifier does not decide whether to hold, it
//! decides who pays to get through:
//!
//! - important: delivered at once, free. A login code nobody can pay for is a
//!   login code that never arrives, so this tier is never held.
//! - human: held. Proving personhood clears it for nothing.
//! - commercial: held. Assumed to be a machine, so it pays.
//! - dangerous: never delivered, whatever anyone does. Charged as a penalty if
//!   a wallet is attached, and proving personhood does not clear it.
//!
//! Every verdict is a 200 with a [`GatewayVerdict`] in the body, plus context
//! the worker ignores. A non-2xx tells the worker it could not reach us, so
//! only three answers use a status: 401 for a caller without the webhook
//! secret, 404 for an inbox that does not exist (the one status the worker
//! reads as an answer), and 500 for a gateway fault, whose body names the
//! stage that failed (see [`crate::faults`]). A body no field of which can be
//! read is a 400, refused before anything is read or spent.

mod challenge;
mod forwarding;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod tests_support;

use std::error::Error;

use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use postage_core::challenge_email::{ChallengeMailFacts, challenge_mail};
use postage_core::classify::{MailFacts, Verdict, classify_from_headers, extract_urls};
use postage_core::handle::{handle_of, is_ours};
use postage_core::quote_types::QuoteFields;
use postage_core::secret::offered_secret_matches;
use postage_shared::{
    GatewayAction, GatewayNotice, GatewayVerdict, INBOUND_BODY_LIMIT_BYTES, INBOUND_DEADLINE_MS,
    Tier,
};
use serde::{Serialize, Serializer};
use serde_json::Value;

use self::challenge::{HeldMail, IssuedChallenge, issue_challenge};
use self::forwarding::{Candidate, forward_without_challenge};
use super::js::{field, is_truthy, request_json};
use super::{json, refusal};
use crate::app::AppState;
use crate::db::Db;
use crate::db::classifications::{BudgetState, claim_classification, purge_old_classifications};
use crate::db::inboxes::inbox_by_handle;
use crate::faults::{
    BoxError, FaultStage, GatewayFault, configured, configured_without_naming, during,
    fault_response,
};

const SECRET_HEADER: &str = "x-postage-secret";

/// Leaving the route early: with an answer already written, or with a fault
/// that [`fault_response`] answers for.
#[derive(Debug)]
enum Stop {
    Answer(Box<Response>),
    Fault(BoxError),
}

impl<E: Error + Send + Sync + 'static> From<E> for Stop {
    fn from(error: E) -> Self {
        Self::Fault(Box::new(error))
    }
}

fn answer(response: Response) -> Stop {
    Stop::Answer(Box::new(response))
}

/// The message as the worker describes it. `subject` and `body` default to
/// empty, as `?? ""` did.
#[derive(Debug, Clone)]
struct InboundMessage {
    from: String,
    to: String,
    subject: String,
    body: String,
    spf: Option<String>,
    dkim: Option<String>,
    dmarc: Option<String>,
}

/// How long one message may be worked on before the gateway answers with its
/// fault response, shorter than the platform's own cut-off so the answer is
/// ours.
const DEADLINE: Duration = Duration::from_millis(INBOUND_DEADLINE_MS as u64);

pub(crate) async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    post_within(DEADLINE, state, headers, request).await
}

/// [`post`] with the deadline given, so a test need not wait out a minute and
/// a half.
///
/// The body is read here rather than by the `Bytes` extractor, whose 2 MB
/// default refused mail the platform would have let through. A message the
/// worker sent that is over the platform's limit is a 413, as it would be
/// from the platform.
async fn post_within(
    deadline: Duration,
    state: AppState,
    headers: HeaderMap,
    request: Request<Body>,
) -> Response {
    let Ok(body) = axum::body::to_bytes(request.into_body(), INBOUND_BODY_LIMIT_BYTES).await else {
        return refusal(StatusCode::PAYLOAD_TOO_LARGE, "Payload too large");
    };
    let outcome = match tokio::time::timeout(deadline, inbound(&state, &headers, &body)).await {
        Ok(outcome) => outcome,
        Err(_elapsed) => Err(Stop::Fault(Box::new(GatewayFault::new(
            FaultStage::Unexpected,
            "The gateway took too long to answer",
        )))),
    };
    match outcome {
        Ok(response) => response,
        Err(Stop::Answer(response)) => *response,
        Err(Stop::Fault(cause)) => fault_response(state.env(), cause),
    }
}

/// The route proper. Everything that can fail on infrastructure rather than
/// on what the message says is named for the part of the gateway that
/// failed, so the answer can tell a database outage from an unset variable.
async fn inbound(state: &AppState, headers: &HeaderMap, body: &Bytes) -> Result<Response, Stop> {
    // The one setting read before the caller is anyone, and so the one that
    // answers without naming itself: the secret *is* the check, so it cannot
    // be ordered after it.
    let secret = configured_without_naming(state.env(), "MAIL_WEBHOOK_SECRET")?;
    if !offered_secret(headers).is_some_and(|offered| offered_secret_matches(&offered, &secret)) {
        return Err(answer(refusal(StatusCode::UNAUTHORIZED, "Bad secret")));
    }

    // Past the secret the caller is the mail worker, and a variable nobody
    // set is worth naming outright to whoever has to go and set it.
    let app_url = configured(state.env(), "APP_URL")?;
    let message = read_message(body)?;

    let db = during(FaultStage::Database, state.db()).await?;
    let handle = handle_of(&message.to);
    let (destination, wallet) = deliverable_inbox(db, &handle).await?;

    let judged = judge(state, db, &message, &handle).await?;
    if let Some(response) = forwarded(state, db, &judged, &handle, &destination).await? {
        return Ok(response);
    }
    challenged(state, db, &judged, &handle, &wallet, &app_url).await
}

/// The inbox's destination and wallet, or the answer for an inbox mail
/// cannot reach.
///
/// An unknown inbox is the one 404. The other two are answered 200 with a
/// verdict, like every other outcome: a non-2xx makes the worker ask the
/// sender's server to retry, for days, over something retrying cannot fix.
async fn deliverable_inbox(db: &Db, handle: &str) -> Result<(String, String), Stop> {
    let Some(inbox) = during(FaultStage::Database, inbox_by_handle(db, handle)).await? else {
        let wire = GatewayVerdict::reject("unknown_inbox", None);
        return Err(answer(json(StatusCode::NOT_FOUND, &wire)));
    };
    let Some(wallet) = inbox.wallet.filter(|wallet| !wallet.is_empty()) else {
        return Err(refuse_mail(
            "no_wallet",
            "That address cannot receive mail yet: nobody has claimed it fully.",
        ));
    };
    // Forwarding to our own domain sends the message straight back in, and
    // every lap spends a classify call, a chain read and a challenge row.
    if is_ours(&inbox.destination) {
        return Err(refuse_mail(
            "loop",
            "That address forwards back to this domain, so nothing can be delivered.",
        ));
    }
    Ok((inbox.destination, wallet))
}

/// The forward answer, when the message goes straight through.
async fn forwarded(
    state: &AppState,
    db: &Db,
    judged: &Judged,
    handle: &str,
    destination: &str,
) -> Result<Option<Response>, Stop> {
    let candidate = Candidate {
        handle,
        sender: &judged.facts.from,
        verdict: &judged.verdict,
        authenticated: judged.authenticated,
        budget_refusal: judged.budget_refusal,
    };
    let forwarded = during(
        FaultStage::Database,
        forward_without_challenge(db, &candidate, state.now()),
    )
    .await?;
    Ok(forwarded.map(|forwarded| {
        let body = Forwarding {
            action: GatewayAction::Forward,
            to: destination,
            reason: &forwarded.reason,
            verdict: VerdictBody::of(&judged.verdict),
        };
        json(StatusCode::OK, &body)
    }))
}

/// A message that will never be delivered, whatever anyone does about it.
/// The worker puts `bounce` in the SMTP refusal, so the sender is told why.
fn refuse_mail(reason: &str, bounce: &str) -> Stop {
    let wire = GatewayVerdict::reject(reason, Some(bounce.to_owned()));
    answer(json(StatusCode::OK, &wire))
}

/// The header carrying the webhook secret, read as `Headers.get` reads it:
/// repeated values joined with a comma and a space.
fn offered_secret(headers: &HeaderMap) -> Option<String> {
    let values: Option<Vec<&str>> = headers
        .get_all(SECRET_HEADER)
        .iter()
        .map(|value| value.to_str().ok())
        .collect();
    values
        .filter(|values| !values.is_empty())
        .map(|values| values.join(", "))
}

/// Reads the worker's JSON, refusing what cannot be read before anything is
/// looked up or spent.
///
/// The TypeScript cast the body and read it unchecked: a body that was not
/// JSON, a JSON `null`, or a field of the wrong type threw, and the catch-all
/// answered a 500 "unexpected" fault, some of them only after the
/// classification budget had been claimed. A missing or falsy `from` or `to`
/// keeps its 400; everything else unreadable is now a 400 too.
fn read_message(body: &Bytes) -> Result<InboundMessage, Stop> {
    let invalid = || answer(refusal(StatusCode::BAD_REQUEST, "Invalid request"));
    let Ok(request) = request_json(body) else {
        return Err(invalid());
    };
    let (from, to) = (field(&request, "from"), field(&request, "to"));
    if !is_truthy(from) || !is_truthy(to) {
        return Err(answer(refusal(
            StatusCode::BAD_REQUEST,
            "from and to are required",
        )));
    }
    let text = |name: &str| optional_text(field(&request, name)).ok_or_else(invalid);
    Ok(InboundMessage {
        from: text("from")?.unwrap_or_default(),
        to: text("to")?.unwrap_or_default(),
        subject: text("subject")?.unwrap_or_default(),
        body: text("body")?.unwrap_or_default(),
        spf: text("spf")?,
        dkim: text("dkim")?,
        dmarc: text("dmarc")?,
    })
}

/// An optional string field: `Some(None)` when absent or `null`, `None` when
/// it holds anything other than a string.
fn optional_text(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

/// Whether the receiving server could confirm the envelope sender is who it
/// says.
///
/// The allowlist is keyed on that address, so letting an unauthenticated
/// message skip the gate would let anyone through by writing someone else's
/// name on the envelope; and answering a forged sender is backscatter. Not
/// "dkim is not fail": the worker collapses disagreeing
/// Authentication-Results to null, and null must not read as clean. Only an
/// actual verified signature counts.
fn sender_is_authenticated(message: &InboundMessage) -> bool {
    let passed = |value: &Option<String>| value.as_deref() == Some("pass");
    passed(&message.dmarc) || (passed(&message.spf) && passed(&message.dkim))
}

/// What the classifier made of a message, and what it was allowed to spend.
struct Judged {
    facts: MailFacts,
    verdict: Verdict,
    authenticated: bool,
    budget_refusal: Option<BudgetState>,
}

/// Classifies the message, within the hourly budget.
///
/// Read before any pass is honoured: a pass says this sender got through the
/// gate minutes ago, not what they have written since. Past the budget the
/// model is not asked and the message is judged from its headers, held
/// rather than delivered, so a flood is never the way through the gate.
///
/// Only mail whose sender the receiving server confirmed may spend the
/// budget. Anyone can write any address on an envelope, so counting
/// unauthenticated mail let a stranger drain a recipient's pool and push the
/// classifier into the header fallback, where transactional subjects are
/// delivered free. Unauthenticated mail is judged from its headers and held.
async fn judge(
    state: &AppState,
    db: &Db,
    message: &InboundMessage,
    handle: &str,
) -> Result<Judged, Stop> {
    let sender = message.from.to_lowercase();
    let authenticated = sender_is_authenticated(message);
    let facts = MailFacts {
        from: sender,
        to: message.to.clone(),
        subject: message.subject.clone(),
        body: message.body.clone(),
        spf: message.spf.clone(),
        dkim: message.dkim.clone(),
        dmarc: message.dmarc.clone(),
        urls: extract_urls(&message.body),
    };

    // Dropped on the way past rather than by a job nobody runs, so the table
    // the budget counts over stays the size of one hour.
    let now = state.now();
    during(FaultStage::Database, purge_old_classifications(db, now)).await?;

    let budget_refusal = if authenticated {
        during(
            FaultStage::Database,
            claim_classification(db, handle, &facts.from, now),
        )
        .await?
    } else {
        Some(BudgetState::SpentBySender)
    };
    let verdict = match budget_refusal {
        Some(_) => classify_from_headers(&facts),
        None => state.classifier().classify(&facts).await,
    };
    Ok(Judged {
        facts,
        verdict,
        authenticated,
        budget_refusal,
    })
}

/// Prices and records a challenge, then answers with the hold, or with the
/// refusal dangerous mail gets.
async fn challenged(
    state: &AppState,
    db: &Db,
    judged: &Judged,
    handle: &str,
    wallet: &str,
    app_url: &str,
) -> Result<Response, Stop> {
    let mail = HeldMail {
        handle,
        sender: &judged.facts.from,
        subject: &judged.facts.subject,
        verdict: &judged.verdict,
        wallet,
        app_url,
    };
    let issued = issue_challenge(state, db, &mail)
        .await
        .map_err(Stop::Fault)?;
    let context = Context::of(&judged.verdict, &issued);
    let reason = judged.verdict.tier;

    let Some(held_until) = issued.held_until else {
        return Ok(rejected(reason, &issued, context));
    };

    // Present only when we can write back without mailing a stranger whose
    // name was borrowed. Without it the worker refuses the message instead,
    // and the link travels in the bounce the sender's own server writes them.
    let notice = judged
        .authenticated
        .then(|| notice(state, &mail, &issued, held_until));
    let body = Held {
        action: GatewayAction::Hold,
        reason,
        token: &issued.token,
        held_until,
        notice,
        bounce: format!(
            "Held, not lost: say whether a person or a machine wrote this and we deliver the message you already sent - {}",
            issued.challenge_url
        ),
        context,
    };
    Ok(json(StatusCode::OK, &body))
}

/// Dangerous mail, refused inside the SMTP session with the link a sender
/// who thinks it a mistake can use.
fn rejected(reason: Tier, issued: &IssuedChallenge, context: Context<'_>) -> Response {
    let bounce = format!(
        "Not delivered: this looks like an attempt to deceive the recipient, and paying will not change that. If it is a mistake, say so at {}",
        issued.challenge_url
    );
    let body = Rejected {
        action: GatewayAction::Reject,
        reason,
        bounce,
        context,
    };
    json(StatusCode::OK, &body)
}

/// The one message a held sender is written back, which the worker sends.
fn notice(
    state: &AppState,
    mail: &HeldMail<'_>,
    issued: &IssuedChallenge,
    held_until: i64,
) -> GatewayNotice {
    let facts = ChallengeMailFacts {
        handle: mail.handle.to_owned(),
        subject: mail.subject.to_owned(),
        amount: issued.price,
        reasons: issued.reasons.clone(),
        challenge_url: issued.challenge_url.clone(),
        app_url: mail.app_url.to_owned(),
        held_until,
    };
    challenge_mail(&facts, state.now())
}

/// The classifier's verdict as the TypeScript serialized it: a whole
/// confidence is written `1`, not `1.0`.
#[derive(Serialize)]
struct VerdictBody<'a> {
    tier: Tier,
    #[serde(serialize_with = "js_number")]
    confidence: f64,
    reasons: &'a [String],
    degraded: bool,
}

impl<'a> VerdictBody<'a> {
    fn of(verdict: &'a Verdict) -> Self {
        Self {
            tier: verdict.tier,
            confidence: verdict.confidence,
            reasons: &verdict.reasons,
            degraded: verdict.degraded,
        }
    }
}

/// A number as `JSON.stringify` writes it, for the values a confidence takes.
fn js_number<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    const SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    if value.fract() == 0.0 && value.abs() <= SAFE_INTEGER {
        return serializer.serialize_i64(*value as i64);
    }
    serializer.serialize_f64(*value)
}

/// `{ ...wire, verdict }`: `verdict` rides along for tests and logs; the
/// worker depends only on the wire fields.
#[derive(Serialize)]
struct Forwarding<'a> {
    action: GatewayAction,
    to: &'a str,
    reason: &'a str,
    verdict: VerdictBody<'a>,
}

/// Extra context on a challenged message, for this route's tests and for
/// logs; not part of the wire contract.
#[derive(Serialize)]
struct Context<'a> {
    verdict: VerdictBody<'a>,
    price: String,
    reasons: &'a [String],
    challenge_url: &'a str,
    quote: &'a QuoteFields,
}

impl<'a> Context<'a> {
    fn of(verdict: &'a Verdict, issued: &'a IssuedChallenge) -> Self {
        Self {
            verdict: VerdictBody::of(verdict),
            price: issued.price.to_string(),
            reasons: &issued.reasons,
            challenge_url: &issued.challenge_url,
            quote: &issued.quote,
        }
    }
}

/// `{ ...wire, ...context }` for a held message. `notice` is written as
/// `null`, not omitted, when the sender is not written back.
#[derive(Serialize)]
struct Held<'a> {
    action: GatewayAction,
    reason: Tier,
    token: &'a str,
    held_until: i64,
    notice: Option<GatewayNotice>,
    bounce: String,
    #[serde(flatten)]
    context: Context<'a>,
}

/// `{ ...wire, ...context }` for dangerous mail.
#[derive(Serialize)]
struct Rejected<'a> {
    action: GatewayAction,
    reason: Tier,
    bounce: String,
    #[serde(flatten)]
    context: Context<'a>,
}
