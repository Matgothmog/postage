//! `GET` and `POST /api/inbox` (`web/src/app/api/inbox/route.ts`): a signed-in
//! user reading their inbox, and anyone starting a claim on a handle.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use postage_core::handle::{
    HANDLE_MAX_LENGTH, HANDLE_MIN_LENGTH, has_handle_shape, has_repeated_dot, is_ours,
};
use postage_core::privy::PrivyIdentity;
use postage_core::quote::parse_address;
use postage_core::secret::VerificationKey;
use postage_core::verification::{CODE_TTL_SECONDS, generate_code, hash_code};
use postage_core::wallet_proof::{IDENTITY_TOKEN_HEADER, OfferedProof, read_proof};
use rand_core::{OsRng, TryRngCore};
use serde::Serialize;
use serde_json::Value;

use super::js::{field, is_js_whitespace, js_length, request_json};
use super::{Exit, RouteResult, invalid_request, json, refuse};
use crate::app::AppState;
use crate::auth::{WalletAuth, claim_statement, header_text, read_statement};
use crate::claims::settle_claim;
use crate::config::message_id_secret;
use crate::db::claims::{
    ClaimSendLimits, NewClaim, attach_destination_to_claim, claim_by_handle, purge_old_claim_sends,
    recent_claims_from, recent_claims_to, record_claim_send_within, start_claim_if_free,
};
use crate::db::inboxes::{Inbox, inbox_by_handle, inbox_by_wallet};
use crate::db::{Db, DbError};
use crate::faults::{reason_chain, redact};
use crate::log;

/// Addresses the service itself relies on. A user holding `hello` would
/// receive every reply and bounce to our own verification mail, which is where
/// other people's codes are quoted; postmaster and abuse are required to reach
/// us by RFC 2142 rather than a user.
const RESERVED: [&str; 15] = [
    "hello",
    "postmaster",
    "abuse",
    "admin",
    "administrator",
    "noreply",
    "no-reply",
    "support",
    "help",
    "info",
    "security",
    "billing",
    "mailer-daemon",
    "webmaster",
    "postage",
];

/// One address can only be asked to confirm so often. Each claim makes at
/// least one stranger mail it, from a sender whose reputation we depend on, so
/// without this an unauthenticated loop is an email bomb aimed at anyone.
const MAX_CLAIMS_PER_DESTINATION: i64 = 3;

/// And one wallet can only start so many, whatever addresses it names: each
/// claim that reaches a code registers a Cloudflare destination, which the
/// account has a hard cap on and nothing deletes. Set above what anyone needs;
/// a ceiling on farming, not a budget anybody should feel.
const MAX_CLAIMS_PER_WALLET: i64 = 5;

const THROTTLE_WINDOW_SECONDS: i64 = 60 * 60;

const THROTTLE: ClaimSendLimits = ClaimSendLimits {
    window_seconds: THROTTLE_WINDOW_SECONDS,
    per_destination: MAX_CLAIMS_PER_DESTINATION,
    per_wallet: MAX_CLAIMS_PER_WALLET,
};

const INVALID_WALLET: &str = "A valid wallet is required";
const TAKEN: &str = "That handle is taken";
const BEING_CLAIMED: &str = "Someone is claiming that handle right now. Try again in a few minutes";
const DESTINATION_THROTTLED: &str =
    "That address has been asked to confirm too many times. Try again later";
const WALLET_THROTTLED: &str =
    "That wallet has claimed too many addresses this hour. Try again later";

#[derive(Serialize)]
struct InboxView {
    inbox: Option<Inbox>,
}

/// What a started claim answers, in the order the TypeScript wrote it. Only
/// a claim waiting on a mailed code says how long the code lasts.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimProgress<'a> {
    status: &'static str,
    handle: &'a str,
    destination: &'a str,
    code_verified: bool,
    cloudflare_verified: bool,
    live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<i64>,
}

/// Where a signed-in user sees their inbox. The row holds the address they
/// actually read, so it is returned only to someone who can prove the wallet
/// is theirs rather than to anyone who knows it: wallets are public.
pub(crate) async fn get(State(state): State<AppState>, headers: HeaderMap) -> RouteResult {
    let offered = read_proof(|name| header_text(&headers, name));

    let identity = state
        .privy()
        .read_identity(offered.identity_token.as_deref())
        .await;
    if let Some(identity) = identity {
        let db = state.db().await?;
        for wallet in &identity.wallets {
            if let Some(inbox) = inbox_by_wallet(db, wallet).await? {
                return Ok(json(StatusCode::OK, &InboxView { inbox: Some(inbox) }));
            }
        }
        return Ok(json(StatusCode::OK, &InboxView { inbox: None }));
    }

    let Some(wallet) = offered
        .wallet
        .as_deref()
        .filter(|wallet| parse_address(wallet).is_ok())
    else {
        return Err(refuse(StatusCode::BAD_REQUEST, INVALID_WALLET));
    };
    if !proves(&state, &offered, |at| read_statement(wallet, at)).await? {
        return Err(refuse(
            StatusCode::UNAUTHORIZED,
            "Sign in again to read this inbox",
        ));
    }

    let inbox = inbox_by_wallet(state.db().await?, wallet).await?;
    Ok(json(StatusCode::OK, &InboxView { inbox }))
}

/// Whether the signature half of `offered` proves its wallet.
///
/// A database that cannot be reached is a refusal, as it was in the
/// TypeScript, where the nonce ledger's failure was caught and read as an
/// unproven wallet. The log line is what tells an operator it was an outage.
async fn proves<S>(state: &AppState, offered: &OfferedProof, statement: S) -> Result<bool, Exit>
where
    S: Fn(f64) -> String,
{
    let nonce_key = state.wallet_nonce_key()?;
    let db = match state.db().await {
        Ok(db) => db,
        Err(error) => {
            let reason = redact(state.env(), &reason_chain(&error));
            log::error(
                "spent wallet nonce ledger unavailable",
                &[("reason", &reason)],
            );
            return Ok(false);
        }
    };
    let auth = WalletAuth {
        db,
        nonce_key: &nonce_key,
    };
    Ok(auth.proves_wallet(offered, statement, state.now()).await)
}

/// The body of a claim, every field checked for its type before anything
/// happens. The TypeScript cast it and let a handle or destination that was
/// not a string throw a bare 500, and a nonce that was not one throw past the
/// proof; all are refused here instead.
#[derive(Debug)]
struct ClaimRequest<'a> {
    handle: Option<&'a str>,
    destination: Option<&'a str>,
    wallet: &'a str,
    issued_at: f64,
    signature: Option<&'a str>,
    /// Carried in the body rather than a header on this one route, because
    /// the claim it belongs to is a body already.
    nonce: Option<&'a str>,
}

fn read_claim(body: &Value) -> Result<ClaimRequest<'_>, Exit> {
    let wallet = match field(body, "wallet") {
        Some(Value::String(wallet)) if parse_address(wallet).is_ok() => wallet,
        _ => return Err(refuse(StatusCode::BAD_REQUEST, INVALID_WALLET)),
    };
    let issued_at = match field(body, "issuedAt") {
        None | Some(Value::Null) => f64::NAN,
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(_) => return Err(invalid_request()),
    };
    Ok(ClaimRequest {
        handle: optional_text(body, "handle")?,
        destination: optional_text(body, "destination")?,
        wallet,
        issued_at,
        signature: optional_text(body, "signature")?,
        nonce: optional_text(body, "nonce")?,
    })
}

/// A field that may be left out or null, and is otherwise a string.
fn optional_text<'a>(body: &'a Value, name: &str) -> Result<Option<&'a str>, Exit> {
    match field(body, name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(invalid_request()),
    }
}

/// Starts a claim on the handle. Nothing is forwarded yet, and the handle
/// only becomes an inbox once two separate things are true: the caller holds
/// the wallet, and can read the address the handle will point at.
///
/// Privy's identity token carries both already, so a signed-in user claiming
/// their own address answers nothing more. Everyone else signs for the wallet
/// and is mailed a code for the address, because Cloudflare's verification is
/// shared across the whole account and cannot stand in for it.
pub(crate) async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> RouteResult {
    let Ok(body) = request_json(&body) else {
        return Err(invalid_request());
    };
    let request = read_claim(&body)?;

    let signed_in = session(&state, &headers, request.wallet).await;
    let name = request.handle.unwrap_or_default().to_lowercase();
    let email = signed_in
        .as_ref()
        .and_then(|identity| identity.email.as_deref());
    let address = request
        .destination
        .or(email)
        .unwrap_or_default()
        .to_lowercase();

    if let Some(rejection) = validate(&name, &address) {
        return Err(refuse(StatusCode::BAD_REQUEST, rejection));
    }
    if signed_in.is_none() && !proves_claim(&state, &request, &name, &address).await? {
        return Err(refuse(
            StatusCode::UNAUTHORIZED,
            "Sign the request with the wallet you are claiming for",
        ));
    }

    let db = state.db().await?;
    let now = state.now();
    if let Some(taken) = unavailable_to(db, &name, request.wallet, now).await? {
        return Err(refuse(StatusCode::CONFLICT, taken));
    }
    throttle(db, &address, request.wallet, now).await?;

    let claim = Claimant {
        handle: &name,
        destination: &address,
        wallet: request.wallet,
    };
    if email == Some(address.as_str()) {
        start_confirmed(&state, db, &claim, now).await
    } else {
        start_with_code(&state, db, &claim, now).await
    }
}

/// A signed-in session that owns the wallet it is claiming for.
async fn session(state: &AppState, headers: &HeaderMap, wallet: &str) -> Option<PrivyIdentity> {
    let token = header_text(headers, IDENTITY_TOKEN_HEADER);
    let identity = state.privy().read_identity(token.as_deref()).await?;
    identity
        .wallets
        .contains(&wallet.to_lowercase())
        .then_some(identity)
}

async fn proves_claim(
    state: &AppState,
    request: &ClaimRequest<'_>,
    name: &str,
    address: &str,
) -> Result<bool, Exit> {
    let offered = OfferedProof {
        identity_token: None,
        wallet: Some(request.wallet.to_owned()),
        issued_at: request.issued_at,
        signature: request.signature.map(str::to_owned),
        nonce: request.nonce.map(str::to_owned),
    };
    let wallet = request.wallet;
    proves(state, &offered, |at| {
        claim_statement(name, address, wallet, at)
    })
    .await
}

fn validate(handle: &str, destination: &str) -> Option<&'static str> {
    let length = js_length(handle);
    if !(HANDLE_MIN_LENGTH..=HANDLE_MAX_LENGTH).contains(&length) || !has_handle_shape(handle) {
        return Some(
            "Pick 2-31 characters: letters, digits, dot, dash, not starting or ending with punctuation",
        );
    }
    if has_repeated_dot(handle) {
        return Some("Two dots in a row is not a valid address");
    }
    if RESERVED.contains(&handle) {
        return Some("That name is reserved");
    }

    if !looks_like_email(destination) {
        return Some("A valid destination address is required");
    }
    if is_ours(destination) {
        return Some("Forward to an inbox you already read, not back to Postage");
    }
    None
}

/// `/^[^@\s]+@[^@\s]+\.[^@\s]+$/`: one `@`, no JavaScript whitespace, and a
/// dot in the domain with something either side of it.
fn looks_like_email(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    let plain =
        |part: &str| !part.is_empty() && !part.contains('@') && !part.chars().any(is_js_whitespace);
    plain(local)
        && plain(domain)
        && domain
            .char_indices()
            .any(|(index, character)| character == '.' && index > 0 && index + 1 < domain.len())
}

/// A handle is free if nobody owns it and nobody else is part way through
/// claiming it. An abandoned claim releases it, but one whose owner has
/// already answered the code does not: losing that would undo work they can
/// see.
async fn unavailable_to(
    db: &Db,
    handle: &str,
    wallet: &str,
    now: i64,
) -> Result<Option<&'static str>, DbError> {
    let wallet = wallet.to_lowercase();
    let owner = inbox_by_handle(db, handle).await?;
    if owner.is_some_and(|owner| owner.wallet.as_deref() != Some(wallet.as_str())) {
        return Ok(Some(TAKEN));
    }

    let Some(claim) = claim_by_handle(db, handle).await? else {
        return Ok(None);
    };
    if claim.wallet == wallet {
        return Ok(None);
    }
    if claim.code_verified_at.is_some() {
        return Ok(Some(TAKEN));
    }
    Ok((claim.expires_at > now).then_some(BEING_CLAIMED))
}

/// Refuses a claim past either ceiling, and otherwise counts it. Counted
/// before anything is sent rather than after: a claim that mails the address
/// and then fails has still mailed it.
async fn throttle(db: &Db, address: &str, wallet: &str, now: i64) -> Result<(), Exit> {
    // Dropped on the way past, so the table both counts run over stays the
    // size of the window rather than growing a row per attempt forever.
    purge_old_claim_sends(db, THROTTLE_WINDOW_SECONDS, now).await?;

    if let Some(refusal) = over_limit(db, address, wallet, now).await? {
        return Err(refuse(StatusCode::TOO_MANY_REQUESTS, refusal));
    }
    if record_claim_send_within(db, address, wallet, THROTTLE, now).await? {
        return Ok(());
    }
    // Concurrent claims took the last slot between the count and the write.
    let refusal = over_limit(db, address, wallet, now)
        .await?
        .unwrap_or(WALLET_THROTTLED);
    Err(refuse(StatusCode::TOO_MANY_REQUESTS, refusal))
}

async fn over_limit(
    db: &Db,
    address: &str,
    wallet: &str,
    now: i64,
) -> Result<Option<&'static str>, DbError> {
    if recent_claims_to(db, address, THROTTLE_WINDOW_SECONDS, now).await?
        >= MAX_CLAIMS_PER_DESTINATION
    {
        return Ok(Some(DESTINATION_THROTTLED));
    }
    let from_wallet = recent_claims_from(db, wallet, THROTTLE_WINDOW_SECONDS, now).await?;
    Ok((from_wallet >= MAX_CLAIMS_PER_WALLET).then_some(WALLET_THROTTLED))
}

/// Who is claiming what, once every check has passed.
#[derive(Debug, Clone, Copy)]
struct Claimant<'a> {
    handle: &'a str,
    destination: &'a str,
    wallet: &'a str,
}

impl Claimant<'_> {
    fn new_claim(&self, code_hash: String, now: i64) -> NewClaim {
        NewClaim {
            handle: self.handle.to_owned(),
            destination: self.destination.to_owned(),
            wallet: self.wallet.to_owned(),
            code_hash,
            expires_at: now + CODE_TTL_SECONDS,
            // Cloudflare is not told about the address until it is confirmed:
            // registering now would have it send its own mail at the same
            // moment as ours, two emails and two instructions at once.
            cf_address_id: None,
            cf_verified_at: None,
        }
    }
}

/// The long way: a code mailed to the address, and the claim waiting on it.
async fn start_with_code(
    state: &AppState,
    db: &Db,
    claimant: &Claimant<'_>,
    now: i64,
) -> RouteResult {
    // Derived before the mail goes out, so a deployment missing its secret
    // fails without having mailed a code nobody can ever enter.
    let key = verification_key(state)?;
    let code = generate_code(&mut OsRng.unwrap_err());
    if let Err(detail) = send_code(state, claimant, &code).await {
        return Err(refuse(StatusCode::BAD_GATEWAY, &detail));
    }

    let claim = claimant.new_claim(hash_code(&key, claimant.handle, &code), now);
    if !start_claim_if_free(db, &claim, None, now).await? {
        return Err(lost_race(db, claimant, now).await?);
    }

    Ok(json(
        StatusCode::OK,
        &ClaimProgress {
            status: "pending",
            handle: claimant.handle,
            destination: claimant.destination,
            code_verified: false,
            cloudflare_verified: false,
            live: false,
            expires_in: Some(CODE_TTL_SECONDS),
        },
    ))
}

/// The short way: Privy has already confirmed the address, so the claim
/// starts with its code half done and only Cloudflare's own link is left.
async fn start_confirmed(
    state: &AppState,
    db: &Db,
    claimant: &Claimant<'_>,
    now: i64,
) -> RouteResult {
    // A code hash is still required, and the hash of a code nobody was sent
    // can never be matched, so the confirm route stays shut rather than open
    // to anything. It also names this start, for the attach below.
    let code_hash = hash_code(
        &verification_key(state)?,
        claimant.handle,
        &generate_code(&mut OsRng.unwrap_err()),
    );
    let claim = claimant.new_claim(code_hash, now);
    if !start_claim_if_free(db, &claim, Some(now), now).await? {
        return Err(lost_race(db, claimant, now).await?);
    }

    register_with_cloudflare(state, db, claimant, &claim.code_hash, now).await;

    let settled = settle_claim(db, state.cloudflare(), claimant.handle, now)
        .await
        .ok()
        .flatten();
    let cloudflare_verified = settled
        .as_ref()
        .is_some_and(|state| state.cloudflare_verified);
    let live = settled.as_ref().is_some_and(|state| state.live);
    Ok(json(
        StatusCode::OK,
        &ClaimProgress {
            status: if live { "live" } else { "pending" },
            handle: claimant.handle,
            destination: claimant.destination,
            code_verified: true,
            cloudflare_verified,
            live,
            expires_in: None,
        },
    ))
}

/// Registers the address with Cloudflare and records it on the claim this
/// request started. Failing here is handled by the poller, which will try
/// again.
async fn register_with_cloudflare(
    state: &AppState,
    db: &Db,
    claimant: &Claimant<'_>,
    code_hash: &str,
    now: i64,
) {
    let Ok(registered) = state
        .cloudflare()
        .ensure_destination(claimant.destination)
        .await
    else {
        return;
    };
    let _ = attach_destination_to_claim(
        db,
        claimant.handle,
        code_hash,
        &registered.id,
        registered.verified_at,
        now,
    )
    .await;
}

fn verification_key(state: &AppState) -> Result<VerificationKey, Exit> {
    Ok(VerificationKey::derive(&message_id_secret(
        state.env().lookup(),
    )?)?)
}

/// Mails the code, or says why not. Missing settings are a failed send,
/// worded as `required()` worded them inside the TypeScript's fetch.
async fn send_code(state: &AppState, claimant: &Claimant<'_>, code: &str) -> Result<(), String> {
    match state.mailer() {
        Ok(mailer) => mailer
            .send_verification_code(claimant.destination, claimant.handle, code)
            .await
            .map_err(|error| error.to_string()),
        Err(missing) => Err(missing.to_string()),
    }
}

/// Another request took the handle between the availability check and the
/// write, and the write changed nothing. Answered as the check would answer
/// now.
async fn lost_race(db: &Db, claimant: &Claimant<'_>, now: i64) -> Result<Exit, DbError> {
    let taken = unavailable_to(db, claimant.handle, claimant.wallet, now)
        .await?
        .unwrap_or(BEING_CLAIMED);
    Ok(refuse(StatusCode::CONFLICT, taken))
}

#[cfg(test)]
mod tests;
