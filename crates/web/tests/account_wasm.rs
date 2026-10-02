//! Mounts the real `Account` over a scripted Privy SDK and a scripted
//! `fetch`, in a real browser. Re-expresses the behaviours the 12 source-regex
//! tests in `web/src/app/Account.test.ts` pinned, as rendered behaviour. Run:
//! `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!  cargo test -p postage-web --target wasm32-unknown-unknown --test account_wasm`

#![cfg(target_arch = "wasm32")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use leptos::prelude::*;
use leptos_router::components::Router;
use postage_web::account::Account;
use postage_web::landing::Landing;
use serde_json::{Value, json};
use support::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const INBOX: &str = "/api/inbox";
const VERIFY: &str = "/api/inbox/verify";

fn inbox_reply() -> Value {
    reply(
        200,
        json!({"inbox": {"handle": "demo", "destination": "demo@example.com",
                         "wallet": WALLET, "created_at": 1}}),
    )
}

fn no_inbox() -> Value {
    reply(200, json!({"inbox": null}))
}

fn unreachable_server() -> Value {
    reply(503, json!({"error": "down"}))
}

/// Chain reads the inbox panel makes once an inbox is on screen.
fn rpc_routes() -> Vec<Value> {
    let rpc = "https://rpc.testnet.arc.network";
    let word = |n: u64| {
        reply_raw(format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":"0x{n:064x}"}}"#
        ))
    };
    vec![
        route_with_body("POST", rpc, "0x543fd313", vec![word(0)]),
        route_with_body("POST", rpc, "0x2aad9987", vec![word(0)]),
        route_with_body("POST", rpc, "0x552c804e", vec![word(10_u64.pow(16))]),
    ]
}

fn reply_raw(body: String) -> Value {
    json!({"status": 200, "body": body})
}

/// Mounts the home page over the given SDK options and routes.
fn open(options: Value, mut routes: Vec<Value>) -> Mount {
    clear_storage();
    routes.extend(rpc_routes());
    install_routes(routes);
    mount(move || {
        start_privy(options);
        view! {
            <Router>
                <Account landing=ViewFn::from(|| view! { <Landing /> }) />
            </Router>
        }
    })
}

fn signed_in_options(token: Option<&str>) -> Value {
    json!({"snapshot": signed_in(WALLET, Some("Tester@Example.com"), token)})
}

// --- the refused-proof rule, as the user sees it ---------------------------

#[wasm_bindgen_test]
async fn a_400_reads_as_not_your_wallet_never_as_a_network_fault() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t")),
        vec![route("GET", INBOX, vec![reply(400, json!({"error": "x"}))])],
    );
    page.shows("That’s not your wallet.").await;
    page.assert_absent("Can’t reach us.");
    assert!(requests_to("POST", INBOX).is_empty());
}

#[wasm_bindgen_test]
async fn a_502_stays_a_network_fault_not_an_unauthorized_one() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t")),
        vec![route("GET", INBOX, vec![html_reply(502)])],
    );
    page.shows("Can’t reach us.").await;
    page.assert_absent("That’s not your wallet.");
}

#[wasm_bindgen_test]
async fn the_error_screen_is_what_a_failed_read_with_nothing_behind_it_shows() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t")),
        vec![route("GET", INBOX, vec![unreachable_server()])],
    );
    page.shows("Can’t reach us.").await;
    page.assert_absent("Showing what we last read.");
    page.assert_absent("Pick your address.");
}

#[wasm_bindgen_test]
async fn while_the_first_read_is_pending_the_page_says_one_moment() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t")),
        vec![route(
            "GET",
            INBOX,
            vec![json!({"gate": "inbox", "status": 200, "body": "{\"inbox\":null}"})],
        )],
    );
    page.shows("One moment.").await;
    open_gate("inbox");
    page.shows("Pick your address.").await;
}

// --- an inbox already read survives a failed refresh (H1) ------------------

#[wasm_bindgen_test]
async fn an_inbox_already_read_survives_a_failed_refresh_as_a_notice() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t1")),
        vec![route(
            "GET",
            INBOX,
            vec![inbox_reply(), unreachable_server()],
        )],
    );
    page.shows("demo@usepostage.com").await;

    // Privy rotates the identity token; the re-read fails.
    emit(&signed_in(WALLET, Some("Tester@Example.com"), Some("t2")));
    page.shows("Showing what we last read.").await;
    page.assert_absent("Can’t reach us.");
    assert!(page.text().contains("demo@usepostage.com"));
    assert!(
        page.text()
            .contains("We couldn’t reach the server just now.")
    );
}

#[wasm_bindgen_test]
async fn a_failed_refresh_over_live_data_is_a_notice_with_a_working_retry() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t1")),
        vec![route(
            "GET",
            INBOX,
            vec![inbox_reply(), reply(401, json!({})), inbox_reply()],
        )],
    );
    page.shows("demo@usepostage.com").await;
    emit(&signed_in(WALLET, Some("Tester@Example.com"), Some("t2")));
    page.shows("Your session went stale.").await;
    assert_eq!(page.buttons_labelled("Try again"), 1);

    page.click("Try again");
    eventually("the notice to clear", || {
        !page.text().contains("Showing what we last read.")
    })
    .await;
    assert!(page.text().contains("demo@usepostage.com"));
}

#[wasm_bindgen_test]
async fn a_claim_in_progress_is_not_hidden_behind_an_unrelated_network_error() {
    quiesce().await;
    clear_storage();
    let page = {
        install_routes(vec![
            route("GET", INBOX, vec![unreachable_server()]),
            route(
                "GET",
                "/api/inbox/verify?handle=demo",
                vec![unreachable_server()],
            ),
        ]);
        store_claim(WALLET, "demo", "demo@example.com", false);
        mount(|| {
            start_privy(signed_in_options(Some("t")));
            view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
        })
    };
    page.shows("One click left").await;
    page.shows("Showing what we last read.").await;
    page.assert_absent("Can’t reach us.");
}

// --- a claim is never sent while the read is what failed (M5) --------------

#[wasm_bindgen_test]
async fn a_hero_claim_waits_out_a_failed_account_read_and_goes_once_a_retry_answers() {
    quiesce().await;
    let page = open(
        json!({
            "snapshot": signed_out(),
            "afterLogin": signed_in(WALLET, Some("Tester@Example.com"), Some("t")),
        }),
        vec![
            route("GET", INBOX, vec![unreachable_server(), no_inbox()]),
            route(
                "POST",
                INBOX,
                vec![reply(
                    200,
                    json!({"status": "started", "handle": "demo", "destination": "tester@example.com",
                           "codeVerified": true, "cloudflareVerified": false, "live": false}),
                )],
            ),
        ],
    );
    page.shows("Make spam pay.").await;
    page.type_into("#hero-handle", "demo");
    eventually("the hero to be enabled", || !page.is_disabled("Claim it")).await;
    page.click("Claim it");

    page.shows("Can’t reach us.").await;
    settle().await;
    assert!(
        requests_to("POST", INBOX).is_empty(),
        "the claim must not be spent behind the error screen"
    );

    page.click("Try again");
    page.shows("One click left").await;
    let sent = requests_to("POST", INBOX);
    assert_eq!(sent.len(), 1);
    let body = request_body(&sent[0]);
    assert_eq!(body["handle"], "demo");
    assert_eq!(body["destination"], "tester@example.com");
    assert_eq!(sent[0]["headers"]["privy-id-token"], "t");

    // The claim is remembered in the TypeScript app's shape.
    let stored = stored_claim().unwrap();
    assert_eq!(stored["wallet"], WALLET);
    assert_eq!(stored["claim"]["handle"], "demo");
    assert_eq!(stored["claim"]["codeVerified"], true);
}

#[wasm_bindgen_test]
async fn a_claim_that_goes_live_at_once_lands_on_the_inbox() {
    quiesce().await;
    let page = open(
        json!({
            "snapshot": signed_out(),
            "afterLogin": signed_in(WALLET, Some("Tester@Example.com"), Some("t")),
        }),
        vec![
            route("GET", INBOX, vec![no_inbox(), inbox_reply()]),
            route(
                "POST",
                INBOX,
                vec![reply(
                    200,
                    json!({"status": "live", "handle": "demo",
                    "destination": "tester@example.com", "codeVerified": true,
                    "cloudflareVerified": true, "live": true}),
                )],
            ),
        ],
    );
    page.type_into("#hero-handle", "demo");
    eventually("the hero to be enabled", || !page.is_disabled("Claim it")).await;
    page.click("Claim it");
    page.shows("Cash out").await;
    assert!(page.text().contains("demo@usepostage.com"));
    assert!(stored_claim().is_none());
}

#[wasm_bindgen_test]
async fn a_claim_without_an_identity_token_carries_a_signature_in_the_body() {
    quiesce().await;
    let page = open(
        json!({"snapshot": signed_in(WALLET, None, None), "signature": "0xsig"}),
        vec![
            route(
                "POST",
                "/api/wallet-nonce",
                vec![reply(200, json!({"nonce": "n1"}))],
            ),
            route("GET", INBOX, vec![no_inbox()]),
            route(
                "POST",
                INBOX,
                vec![reply(
                    200,
                    json!({"handle": "demo", "destination": "other@example.com",
                    "codeVerified": false, "cloudflareVerified": false, "live": false}),
                )],
            ),
        ],
    );
    page.shows("Pick your address.").await;
    page.type_into("#handle", "demo");
    page.type_into("#destination", "other@example.com");
    page.shows("We will email a code there first").await;
    eventually("the form to be ready", || !page.is_disabled("Claim it")).await;
    page.click("Claim it");
    page.shows("One click left").await;

    let body = request_body(&requests_to("POST", INBOX)[0]);
    assert_eq!(body["signature"], "0xsig");
    assert_eq!(body["nonce"], "n1");
    assert!(body["issuedAt"].is_i64() || body["issuedAt"].is_u64());
    let signed = calls_to("signMessage");
    assert!(signed.iter().any(|call| {
        call["input"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("Postage: claim an address") && m.ends_with("Nonce: n1"))
    }));
}

#[wasm_bindgen_test]
async fn a_refused_claim_shows_the_servers_words_on_the_form() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t")),
        vec![
            route("GET", INBOX, vec![no_inbox()]),
            route(
                "POST",
                INBOX,
                vec![reply(409, json!({"error": "That handle is taken"}))],
            ),
        ],
    );
    page.shows("Pick your address.").await;
    page.type_into("#handle", "demo");
    page.click("Claim it");
    page.shows("That handle is taken").await;
    assert!(!page.is_disabled("Claim it"));
}

// --- sign-out and whose claim it is (H2) -----------------------------------

#[wasm_bindgen_test]
async fn signing_out_forgets_the_claim_this_browser_was_holding() {
    quiesce().await;
    clear_storage();
    store_claim(WALLET, "demo", "demo@example.com", false);
    install_routes(vec![
        route("GET", INBOX, vec![no_inbox()]),
        route(
            "GET",
            "/api/inbox/verify?handle=demo",
            vec![reply(
                200,
                json!({
            "codeVerified": false, "cloudflareVerified": false, "live": false}),
            )],
        ),
    ]);
    let page = mount(|| {
        start_privy(signed_in_options(Some("t")));
        view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
    });
    page.shows("One click left").await;
    assert!(stored_claim().is_some());

    page.click("Sign out");
    eventually("the slot to be emptied", || stored_claim().is_none()).await;
    page.shows("Make spam pay.").await;
    assert_eq!(calls_to("logout").len(), 1);
}

#[wasm_bindgen_test]
async fn a_claim_another_wallet_left_behind_is_neither_shown_polled_nor_cleared() {
    quiesce().await;
    clear_storage();
    store_claim(OTHER_WALLET, "theirs", "them@example.com", false);
    install_routes(vec![route("GET", INBOX, vec![no_inbox()])]);
    let page = mount(|| {
        start_privy(signed_in_options(Some("t")));
        view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
    });
    page.shows("Pick your address.").await;
    settle().await;
    page.assert_absent("theirs");
    assert!(requests_to("GET", "/api/inbox/verify?handle=theirs").is_empty());
    assert_eq!(stored_claim().unwrap()["claim"]["handle"], "theirs");
}

#[wasm_bindgen_test]
async fn the_previous_wallets_inbox_is_not_shown_to_the_next_one() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t1")),
        vec![route(
            "GET",
            INBOX,
            vec![inbox_reply(), unreachable_server()],
        )],
    );
    page.shows("demo@usepostage.com").await;

    emit(&signed_out());
    emit(&signed_in(OTHER_WALLET, Some("b@example.com"), Some("t9")));
    page.shows("Can’t reach us.").await;
    page.assert_absent("demo@usepostage.com");
}

// --- the stale error and the stored claim ----------------------------------

#[wasm_bindgen_test]
async fn a_successful_retry_with_no_inbox_clears_a_stale_error_so_the_form_is_reachable() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t")),
        vec![route("GET", INBOX, vec![unreachable_server(), no_inbox()])],
    );
    page.shows("Can’t reach us.").await;
    page.click("Try again");
    page.shows("Pick your address.").await;
    page.assert_absent("Can’t reach us.");
}

#[wasm_bindgen_test]
async fn a_stored_claim_the_server_no_longer_holds_is_dropped_on_load() {
    quiesce().await;
    clear_storage();
    store_claim(WALLET, "demo", "demo@example.com", false);
    install_routes(vec![
        route("GET", INBOX, vec![no_inbox()]),
        route(
            "GET",
            "/api/inbox/verify?handle=demo",
            vec![reply(404, json!({"error": "gone"}))],
        ),
    ]);
    let page = mount(|| {
        start_privy(signed_in_options(Some("t")));
        view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
    });
    page.shows("Pick your address.").await;
    assert!(stored_claim().is_none());
}

#[wasm_bindgen_test]
async fn a_stored_claim_that_could_not_be_checked_stays_on_screen() {
    quiesce().await;
    clear_storage();
    store_claim(WALLET, "demo", "demo@example.com", true);
    install_routes(vec![
        route("GET", INBOX, vec![no_inbox()]),
        route(
            "GET",
            "/api/inbox/verify?handle=demo",
            vec![reply(503, json!({}))],
        ),
    ]);
    let page = mount(|| {
        start_privy(signed_in_options(Some("t")));
        view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
    });
    page.shows("One click left").await;
    settle().await;
    assert!(stored_claim().is_some());
    assert!(page.text().contains("Waiting on Cloudflare."));
}

#[wasm_bindgen_test]
async fn verifying_a_stored_claim_asks_the_poll_endpoint_with_the_handle_url_encoded() {
    quiesce().await;
    install_routes(vec![route(
        "GET",
        "/api/inbox/verify?handle=a%20b",
        vec![reply(
            200,
            json!({"codeVerified": true, "cloudflareVerified": false, "live": false}),
        )],
    )]);
    let stored = postage_web::pending_claim::ClaimProgress {
        handle: "a b".to_owned(),
        destination: "d@example.com".to_owned(),
        code_verified: false,
        cloudflare_verified: false,
    };
    let outcome = postage_web::pending_claim::verify_pending_claim(&stored).await;
    assert_eq!(
        requests_to("GET", "/api/inbox/verify?handle=a%20b").len(),
        1
    );
    assert!(matches!(
        outcome,
        postage_web::pending_claim::PendingClaimOutcome::Pending(claim) if claim.code_verified
    ));
    let _ = VERIFY;
}

// --- answers that arrive after the user moved on (#37, #38) -----------------

fn inbox_reply_for(handle: &str) -> Value {
    reply(
        200,
        json!({"inbox": {"handle": handle, "destination": "demo@example.com",
                         "wallet": WALLET, "created_at": 1}}),
    )
}

fn held_reply(gate: &str, mut response: Value) -> Value {
    response["gate"] = json!(gate);
    response
}

/// Two reads leave in order and answer in the opposite order: the slow, older
/// one must not put its inbox over the newer read's.
#[wasm_bindgen_test]
async fn a_slow_older_read_does_not_overwrite_the_newer_inbox_it_was_overtaken_by() {
    quiesce().await;
    let page = open(
        signed_in_options(Some("t1")),
        vec![route(
            "GET",
            INBOX,
            vec![
                held_reply("old-read", inbox_reply_for("older")),
                inbox_reply_for("newer"),
            ],
        )],
    );
    eventually("the first read to leave", || {
        requests_to("GET", INBOX).len() == 1
    })
    .await;

    // Privy rotates the identity token while the first read is still held.
    emit(&signed_in(WALLET, Some("Tester@Example.com"), Some("t2")));
    page.shows("newer@usepostage.com").await;

    open_gate("old-read");
    settle().await;
    page.assert_absent("older@usepostage.com");
    assert!(page.text().contains("newer@usepostage.com"));
}

/// "Start over" while the restored claim is still being checked: the check's
/// answer is about a claim that is no longer on screen.
#[wasm_bindgen_test]
async fn a_restored_claim_check_that_answers_after_start_over_does_not_bring_the_claim_back() {
    quiesce().await;
    clear_storage();
    store_claim(WALLET, "demo", "demo@example.com", false);
    install_routes(vec![
        route("GET", INBOX, vec![no_inbox()]),
        route(
            "GET",
            "/api/inbox/verify?handle=demo",
            vec![held_reply(
                "restored-check",
                reply(
                    200,
                    json!({"codeVerified": true, "cloudflareVerified": false, "live": false}),
                ),
            )],
        ),
    ]);
    let page = mount(|| {
        start_privy(signed_in_options(Some("t")));
        view! { <Router><Account landing=ViewFn::from(|| view! { <Landing /> }) /></Router> }
    });
    page.shows("One click left").await;
    eventually("the restored claim to be checked", || {
        requests_to("GET", "/api/inbox/verify?handle=demo").len() == 1
    })
    .await;

    page.click("Start over");
    page.shows("Pick your address.").await;
    assert!(stored_claim().is_none());

    open_gate("restored-check");
    settle().await;
    page.assert_absent("One click left");
    assert!(page.text().contains("Pick your address."));
}
