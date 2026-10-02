//! A claim in progress, remembered across page loads. Replaces
//! `web/src/app/pending-claim.ts`.
//!
//! It used to live in component state alone, so reloading while waiting on
//! Cloudflare's email dropped the claimer back onto an empty form, and
//! starting again spends one of the five claims a wallet is allowed in an
//! hour, after which signup stops working for them at all.
//!
//! Nothing secret is kept. The handle is about to be public, the destination
//! is the address whose owner is being asked to confirm it, and the wallet is
//! a public identifier. No token, no signature, no session material.
//!
//! The storage key and the JSON shape are the TypeScript app's, byte for
//! byte in meaning, so a user who is mid-claim when the Rust build replaces
//! the Next one finds their claim still there.

use serde::{Deserialize, Serialize};

use crate::api::{self, ClaimState};
use crate::http::{HttpError, HttpResponse};

/// Where the claim is kept (`localStorage`).
pub const PENDING_CLAIM_KEY: &str = "postage.pending-claim";

/// How far a claim has got. What `POST /api/inbox` answers with (less the
/// fields the strip does not need) and what the strip advances.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimProgress {
    pub handle: String,
    pub destination: String,
    pub code_verified: bool,
    pub cloudflare_verified: bool,
}

/// A claim as it sits in storage. The wallet is the whole reason this shape
/// exists: there is one slot per browser and browsers get shared, so a claim
/// that does not say whose it is would be shown to the next person to sign in
/// here: someone else's handle, someone else's destination, and a code POST
/// the server refuses because the wallet does not match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredClaim {
    pub wallet: String,
    pub claim: ClaimProgress,
}

/// A storage call that failed (storage refused, quota exceeded).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("browser storage failed: {0}")]
pub struct StoreError(pub String);

/// The three methods of `localStorage` this module uses, and nothing else, so
/// the rules below run natively against a fake.
pub trait ClaimStore {
    fn get_item(&self, key: &str) -> Result<Option<String>, StoreError>;
    fn set_item(&self, key: &str, value: &str) -> Result<(), StoreError>;
    fn remove_item(&self, key: &str) -> Result<(), StoreError>;
}

/// The page's `localStorage`.
#[derive(Debug, Clone)]
pub struct BrowserStore(web_sys::Storage);

impl ClaimStore for BrowserStore {
    fn get_item(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.0
            .get_item(key)
            .map_err(|cause| StoreError(format!("{cause:?}")))
    }

    fn set_item(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.0
            .set_item(key, value)
            .map_err(|cause| StoreError(format!("{cause:?}")))
    }

    fn remove_item(&self, key: &str) -> Result<(), StoreError> {
        self.0
            .remove_item(key)
            .map_err(|cause| StoreError(format!("{cause:?}")))
    }
}

/// `localStorage` throws outright in browsers set to refuse site data, so it
/// is reached for once, here, and every caller is handed a `None` it has to
/// deal with instead of an error it would have to guard.
pub fn claim_store() -> Option<BrowserStore> {
    // Site data refused: nothing is remembered, and the claim still works for
    // as long as the tab stays open.
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?
            .local_storage()
            .ok()
            .flatten()
            .map(BrowserStore)
    }
    // Outside a browser there is no `window` to ask (the web-sys calls cannot
    // run), which is the same "no storage here" the answer above gives.
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// An optional store as the trait object the functions below take.
pub fn as_store(store: &Option<BrowserStore>) -> Option<&dyn ClaimStore> {
    store.as_ref().map(|store| store as &dyn ClaimStore)
}

/// Addresses reach this file checksummed from Privy and lower-cased from the
/// server, and they name the same wallet either way.
fn same_wallet(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// A record with no owner is refused rather than adopted by whoever is
/// reading. That is also what the shape written before claims were scoped
/// looks like, so an upgrade forgets one claim in progress: the cheap half of
/// the trade against showing it to a stranger.
fn is_usable(stored: &StoredClaim) -> bool {
    !stored.wallet.is_empty()
        && !stored.claim.handle.is_empty()
        && !stored.claim.destination.is_empty()
}

/// Anything that is not a claim this app wrote reads as no claim at all:
/// truncated JSON, a value from an older shape, a key some other script put
/// there. Rehydrating from half of one would show a strip for a claim the
/// server has never heard of.
pub fn read_stored_claim(store: Option<&dyn ClaimStore>) -> Option<StoredClaim> {
    let raw = store?.get_item(PENDING_CLAIM_KEY).ok().flatten()?;
    let stored: StoredClaim = serde_json::from_str(&raw).ok()?;
    is_usable(&stored).then_some(stored)
}

/// The claim this session is allowed to see, which is only ever the one this
/// wallet stored. Anybody else's reads as absent: not shown, not polled, not
/// acted on. A session with no wallet yet has nothing to match against, so it
/// sees nothing either.
pub fn claim_for(stored: Option<&StoredClaim>, wallet: Option<&str>) -> Option<ClaimProgress> {
    let (stored, wallet) = (stored?, wallet?);
    same_wallet(&stored.wallet, wallet).then(|| stored.claim.clone())
}

pub fn write_pending_claim(
    store: Option<&dyn ClaimStore>,
    wallet: Option<&str>,
    claim: &ClaimProgress,
) {
    let (Some(store), Some(wallet)) = (store, wallet) else {
        return;
    };
    let stored = StoredClaim {
        wallet: wallet.to_owned(),
        claim: claim.clone(),
    };
    let Ok(json) = serde_json::to_string(&stored) else {
        return;
    };
    // Out of quota, or site data refused: the claim survives in state for this
    // page load, which is what it did before there was a store.
    let _ = store.set_item(PENDING_CLAIM_KEY, &json);
}

pub fn clear_pending_claim(store: Option<&dyn ClaimStore>) {
    if let Some(store) = store {
        // A store that will not forget is one nothing was written to either.
        let _ = store.remove_item(PENDING_CLAIM_KEY);
    }
}

/// `encodeURIComponent`: everything except letters, digits and `-_.!~*'()`
/// is percent-encoded as UTF-8 bytes.
pub(crate) fn encode_uri_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The one place the poll endpoint's URL is built. A handle is user input
/// until the server has taken it, and one carrying `&` or `#` would rewrite
/// the query string it is pasted into.
pub fn verify_claim_url(handle: &str) -> String {
    format!(
        "{}?handle={}",
        api::INBOX_VERIFY_PATH,
        encode_uri_component(handle)
    )
}

/// What one answer from `GET /api/inbox/verify` means for the claim it was
/// asked about, from the status alone.
///
/// Both readers of that endpoint decide with this: the reconcile at page load
/// and the strip's four-second poll. They used to disagree (a 404 was "gone"
/// to one and ignored by the other), which left a strip on screen for as long
/// as the tab stayed open after the server had purged the claim.
///
/// "The server says this is gone" and "I could not reach the server" are the
/// two answers that must not be collapsed. Only the first is a reason to stop
/// showing a claim; forgetting one on the second costs its owner one of five
/// tries for a fault that is not theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollVerdict {
    Gone,
    Read,
    Wait,
}

pub fn poll_verdict(status: u16) -> PollVerdict {
    match status {
        404 => PollVerdict::Gone,
        200..300 => PollVerdict::Read,
        _ => PollVerdict::Wait,
    }
}

/// What became of a claim remembered from an earlier page load.
///
/// The four outcomes are four different things to do, and collapsing any two
/// loses something. `Live` finished while the page was closed. `Gone` was
/// never there, or has been promoted and cleared, and must not leave a strip
/// on screen. `Unknown` is a wobble (Cloudflare unreachable, the network
/// down, a gateway page instead of JSON) where forgetting the claim would
/// cost its owner one of five tries for a fault that is not theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingClaimOutcome {
    Live,
    Pending(ClaimProgress),
    Gone,
    Unknown,
}

/// Decides the outcome from the poll endpoint's answer (or its absence).
///
/// The confirmed steps are merged into the stored ones rather than replacing
/// them, the same way the strip's own poll merges a tick: a step that has been
/// confirmed cannot become unconfirmed, and treating one as undone would put
/// the code field back in front of somebody who has already spent their code.
pub fn outcome_of(
    stored: &ClaimProgress,
    answer: Result<HttpResponse, HttpError>,
) -> PendingClaimOutcome {
    let Ok(response) = answer else {
        return PendingClaimOutcome::Unknown;
    };
    match poll_verdict(response.status) {
        PollVerdict::Gone => return PendingClaimOutcome::Gone,
        PollVerdict::Wait => return PendingClaimOutcome::Unknown,
        PollVerdict::Read => {}
    }
    let Ok(state) = serde_json::from_str::<ClaimState>(&response.body) else {
        return PendingClaimOutcome::Unknown;
    };
    if state.live {
        return PendingClaimOutcome::Live;
    }
    PendingClaimOutcome::Pending(ClaimProgress {
        code_verified: stored.code_verified || state.code_verified,
        cloudflare_verified: stored.cloudflare_verified || state.cloudflare_verified,
        ..stored.clone()
    })
}

/// Asks the server what became of `stored`. The endpoint the strip already
/// polls answers it, so nothing new is asked of the API.
pub async fn verify_pending_claim(stored: &ClaimProgress) -> PendingClaimOutcome {
    outcome_of(stored, api::get_claim_state(&stored.handle, None).await)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    const WALLET: &str = "0xAb0000000000000000000000000000000000000a";
    const OTHER_WALLET: &str = "0xCd0000000000000000000000000000000000000c";

    fn claim() -> ClaimProgress {
        ClaimProgress {
            handle: "demo".to_owned(),
            destination: "demo@example.com".to_owned(),
            code_verified: false,
            cloudflare_verified: false,
        }
    }

    #[derive(Default)]
    struct FakeStore(RefCell<HashMap<String, String>>);

    impl FakeStore {
        fn seeded(value: &str) -> Self {
            let store = Self::default();
            store
                .0
                .borrow_mut()
                .insert(PENDING_CLAIM_KEY.to_owned(), value.to_owned());
            store
        }

        fn raw(&self) -> Option<String> {
            self.0.borrow().get(PENDING_CLAIM_KEY).cloned()
        }
    }

    impl ClaimStore for FakeStore {
        fn get_item(&self, key: &str) -> Result<Option<String>, StoreError> {
            Ok(self.0.borrow().get(key).cloned())
        }

        fn set_item(&self, key: &str, value: &str) -> Result<(), StoreError> {
            self.0.borrow_mut().insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn remove_item(&self, key: &str) -> Result<(), StoreError> {
            self.0.borrow_mut().remove(key);
            Ok(())
        }
    }

    /// What a browser configured to refuse site data does: every call fails.
    struct RefusingStore;

    impl ClaimStore for RefusingStore {
        fn get_item(&self, _: &str) -> Result<Option<String>, StoreError> {
            Err(StoreError("storage is disabled".to_owned()))
        }

        fn set_item(&self, _: &str, _: &str) -> Result<(), StoreError> {
            Err(StoreError("storage is disabled".to_owned()))
        }

        fn remove_item(&self, _: &str) -> Result<(), StoreError> {
            Err(StoreError("storage is disabled".to_owned()))
        }
    }

    /// What `read_stored_claim` followed by `claim_for` does together, which
    /// is how every caller reads the slot.
    fn read_for(store: Option<&dyn ClaimStore>, wallet: Option<&str>) -> Option<ClaimProgress> {
        claim_for(read_stored_claim(store).as_ref(), wallet)
    }

    fn answer(status: u16, body: &str) -> Result<HttpResponse, HttpError> {
        Ok(HttpResponse {
            status,
            body: body.to_owned(),
        })
    }

    // claimStore: no localStorage -> null. A native build has no `window`, the
    // same "no storage here" the TypeScript got on the server.
    #[test]
    fn claim_store_hands_back_nothing_where_there_is_no_local_storage_rather_than_failing() {
        assert!(claim_store().is_none());
    }

    #[test]
    fn read_stored_claim_returns_nothing_when_there_is_no_store_to_read() {
        assert_eq!(read_stored_claim(None), None);
    }

    #[test]
    fn read_stored_claim_returns_nothing_when_no_claim_was_ever_stored() {
        assert_eq!(read_stored_claim(Some(&FakeStore::default())), None);
    }

    #[test]
    fn a_stored_claim_is_read_back_whole_so_a_reload_resumes_where_it_left_off() {
        let store = FakeStore::default();
        let progressed = ClaimProgress {
            code_verified: true,
            ..claim()
        };
        write_pending_claim(Some(&store), Some(WALLET), &progressed);
        assert_eq!(read_for(Some(&store), Some(WALLET)), Some(progressed));
    }

    #[test]
    fn read_stored_claim_returns_nothing_for_stored_text_that_is_not_json() {
        let store = FakeStore::seeded("not json");
        assert_eq!(read_stored_claim(Some(&store)), None);
    }

    #[test]
    fn read_stored_claim_returns_nothing_for_a_value_missing_the_fields_the_strip_needs() {
        let store = FakeStore::seeded(&format!(
            r#"{{"wallet":"{WALLET}","claim":{{"handle":"demo"}}}}"#
        ));
        assert_eq!(read_stored_claim(Some(&store)), None);
    }

    #[test]
    fn read_stored_claim_returns_nothing_for_a_value_that_is_not_an_object_at_all() {
        let store = FakeStore::seeded(r#""demo""#);
        assert_eq!(read_stored_claim(Some(&store)), None);
    }

    #[test]
    fn read_stored_claim_returns_nothing_when_the_store_itself_refuses_to_be_read() {
        assert_eq!(read_stored_claim(Some(&RefusingStore)), None);
    }

    /// The shape written before claims were scoped to a wallet. It cannot be
    /// shown to anybody, because there is nobody it can be proved to belong
    /// to.
    #[test]
    fn a_claim_stored_in_the_older_unscoped_shape_is_not_handed_to_anyone() {
        let unscoped = serde_json::to_string(&claim()).unwrap();
        let store = FakeStore::seeded(&unscoped);
        assert_eq!(read_stored_claim(Some(&store)), None);
        assert_eq!(read_for(Some(&store), Some(WALLET)), None);
    }

    #[test]
    fn a_claim_stored_with_an_empty_wallet_is_not_handed_to_anyone() {
        let stored = serde_json::to_string(&StoredClaim {
            wallet: String::new(),
            claim: claim(),
        })
        .unwrap();
        assert_eq!(read_stored_claim(Some(&FakeStore::seeded(&stored))), None);
    }

    /// The cross-user failure this scoping exists for: wallet A claims a
    /// handle and signs out; wallet B signs in on the same browser.
    #[test]
    fn a_claim_wallet_a_left_behind_is_never_handed_to_wallet_b() {
        let store = FakeStore::default();
        write_pending_claim(Some(&store), Some(WALLET), &claim());

        assert_eq!(
            read_for(Some(&store), Some(OTHER_WALLET)),
            None,
            "B must not be shown A's claim"
        );
        assert_eq!(
            read_for(Some(&store), Some(WALLET)),
            Some(claim()),
            "A still gets its own claim back"
        );
    }

    #[test]
    fn claim_for_hands_back_nothing_while_there_is_no_session_to_scope_the_claim_to() {
        let store = FakeStore::default();
        write_pending_claim(Some(&store), Some(WALLET), &claim());
        assert_eq!(read_for(Some(&store), None), None);
    }

    /// Privy hands back a checksummed address and the server answers with a
    /// lower-cased one. They are the same wallet.
    #[test]
    fn a_claim_is_still_its_owners_when_the_address_comes_back_cased_differently() {
        let store = FakeStore::default();
        write_pending_claim(Some(&store), Some(&WALLET.to_lowercase()), &claim());
        assert_eq!(
            read_for(Some(&store), Some(&WALLET.to_uppercase())),
            Some(claim())
        );
    }

    #[test]
    fn write_pending_claim_records_the_wallet_that_made_the_claim() {
        let store = FakeStore::default();
        write_pending_claim(Some(&store), Some(WALLET), &claim());
        assert_eq!(
            read_stored_claim(Some(&store)),
            Some(StoredClaim {
                wallet: WALLET.to_owned(),
                claim: claim()
            })
        );
    }

    #[test]
    fn write_pending_claim_stores_nothing_for_a_session_with_no_wallet_yet() {
        let store = FakeStore::default();
        write_pending_claim(Some(&store), None, &claim());
        assert_eq!(read_stored_claim(Some(&store)), None);
        assert_eq!(store.raw(), None);
    }

    #[test]
    fn write_pending_claim_on_a_store_that_refuses_to_write_is_a_no_op_not_a_crash() {
        write_pending_claim(Some(&RefusingStore), Some(WALLET), &claim());
    }

    #[test]
    fn write_pending_claim_with_no_store_is_a_no_op_not_a_crash() {
        write_pending_claim(None, Some(WALLET), &claim());
    }

    #[test]
    fn clear_pending_claim_forgets_the_stored_claim() {
        let store = FakeStore::default();
        write_pending_claim(Some(&store), Some(WALLET), &claim());
        clear_pending_claim(Some(&store));
        assert_eq!(read_stored_claim(Some(&store)), None);
    }

    #[test]
    fn clear_pending_claim_tolerates_a_missing_store_and_one_that_refuses() {
        clear_pending_claim(None);
        clear_pending_claim(Some(&RefusingStore));
    }

    /// The compatibility the cutover depends on: this is the JSON the
    /// TypeScript app wrote under this key, and what it reads back.
    #[test]
    fn the_stored_json_is_the_typescript_apps_shape_under_the_same_key() {
        assert_eq!(PENDING_CLAIM_KEY, "postage.pending-claim");
        let store = FakeStore::default();
        write_pending_claim(Some(&store), Some(WALLET), &claim());
        let written: serde_json::Value = serde_json::from_str(&store.raw().unwrap()).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "wallet": WALLET,
                "claim": {
                    "handle": "demo",
                    "destination": "demo@example.com",
                    "codeVerified": false,
                    "cloudflareVerified": false
                }
            })
        );

        let from_typescript = FakeStore::seeded(&format!(
            r#"{{"wallet":"{WALLET}","claim":{{"handle":"demo","destination":"demo@example.com","codeVerified":true,"cloudflareVerified":false}}}}"#
        ));
        assert_eq!(
            read_for(Some(&from_typescript), Some(WALLET)),
            Some(ClaimProgress {
                code_verified: true,
                ..claim()
            })
        );
    }

    /// A handle is user input until the server has taken it.
    #[test]
    fn verify_claim_url_encodes_a_handle_that_would_otherwise_rewrite_the_query_string() {
        assert_eq!(
            verify_claim_url("a&b=c"),
            "/api/inbox/verify?handle=a%26b%3Dc"
        );
        assert_eq!(verify_claim_url("a b"), "/api/inbox/verify?handle=a%20b");
        assert_eq!(verify_claim_url("demo"), "/api/inbox/verify?handle=demo");
        assert_eq!(verify_claim_url("é#"), "/api/inbox/verify?handle=%C3%A9%23");
    }

    #[test]
    fn poll_verdict_reads_a_404_as_the_server_saying_the_claim_is_gone() {
        assert_eq!(poll_verdict(404), PollVerdict::Gone);
    }

    #[test]
    fn poll_verdict_reads_an_answer_as_something_to_apply() {
        assert_eq!(poll_verdict(200), PollVerdict::Read);
        assert_eq!(poll_verdict(299), PollVerdict::Read);
    }

    #[test]
    fn poll_verdict_reads_every_other_failure_as_a_reason_to_wait_not_to_forget_the_claim() {
        for status in [503, 500, 429, 400, 301] {
            assert_eq!(poll_verdict(status), PollVerdict::Wait, "{status}");
        }
    }

    // verifyPendingClaim, with the endpoint's answer as an argument. The call
    // itself (URL asked, fetch wiring) is covered in the browser tests.

    #[test]
    fn a_claim_that_finished_while_the_page_was_closed_is_live() {
        let outcome = outcome_of(
            &claim(),
            answer(
                200,
                r#"{"codeVerified":true,"cloudflareVerified":true,"live":true}"#,
            ),
        );
        assert_eq!(outcome, PendingClaimOutcome::Live);
    }

    #[test]
    fn a_claim_the_server_no_longer_holds_is_gone() {
        let outcome = outcome_of(
            &claim(),
            answer(404, r#"{"error":"Nothing is being claimed here"}"#),
        );
        assert_eq!(outcome, PendingClaimOutcome::Gone);
    }

    /// Discarding the claim here would cost its owner one of the five a
    /// wallet gets in an hour, for a fault that is ours and momentary.
    #[test]
    fn cloudflare_being_unreachable_is_unknown_not_a_lost_claim() {
        let outcome = outcome_of(
            &claim(),
            answer(503, r#"{"error":"Waiting on Cloudflare"}"#),
        );
        assert_eq!(outcome, PendingClaimOutcome::Unknown);
    }

    #[test]
    fn a_request_that_never_lands_is_unknown() {
        let outcome = outcome_of(&claim(), Err(HttpError::Network("offline".to_owned())));
        assert_eq!(outcome, PendingClaimOutcome::Unknown);
    }

    #[test]
    fn a_reply_that_is_not_json_is_unknown() {
        let outcome = outcome_of(&claim(), answer(200, "<html>gateway</html>"));
        assert_eq!(outcome, PendingClaimOutcome::Unknown);
    }

    #[test]
    fn the_steps_the_server_has_confirmed_are_taken() {
        let outcome = outcome_of(
            &claim(),
            answer(
                200,
                r#"{"codeVerified":true,"cloudflareVerified":false,"live":false}"#,
            ),
        );
        assert_eq!(
            outcome,
            PendingClaimOutcome::Pending(ClaimProgress {
                code_verified: true,
                ..claim()
            })
        );
    }

    /// A step that has been confirmed cannot become unconfirmed.
    #[test]
    fn a_step_the_stored_claim_had_already_passed_is_never_un_verified() {
        let stored = ClaimProgress {
            code_verified: true,
            ..claim()
        };
        let outcome = outcome_of(
            &stored,
            answer(
                200,
                r#"{"codeVerified":false,"cloudflareVerified":false,"live":false}"#,
            ),
        );
        assert_eq!(outcome, PendingClaimOutcome::Pending(stored));
    }
}
