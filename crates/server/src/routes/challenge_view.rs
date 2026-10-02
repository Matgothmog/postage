//! `GET /api/challenge/{token}`: what the challenge page
//! (`web/src/app/c/[token]/page.tsx`) read on the server before it rendered,
//! for the client-rendered page to fetch instead.
//!
//! Exactly what that page used, and in the same three states. A settled
//! challenge showed "Done." and nothing else, and a challenge whose stored
//! quote no longer parses showed "Dead link."; neither says anything more
//! here. Only an open challenge carries its details: who it is addressed to,
//! whether the message is held, whether it is dangerous, the quote the sender
//! can pay, and the identity mode the page's actions branch on. The `?as=`
//! lane the page also read is the browser's own query string, not data.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use postage_core::quote_types::{StoredQuote, parse_stored_quote};
use serde::Serialize;

use super::{RouteResult, json, refuse};
use crate::app::AppState;
use crate::config::{IdentityMode, identity_mode};
use crate::db::challenges::challenge_by_token;

const DANGEROUS: &str = "dangerous";

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
enum ChallengeView {
    /// Settled already: "That one's dealt with."
    Resolved,
    /// The stored quote is not one the page can offer: "Write again for a
    /// fresh one."
    Dead,
    #[serde(rename_all = "camelCase")]
    Open {
        /// The bare handle; the page shows it as an address.
        handle: String,
        dangerous: bool,
        /// Whether the message itself is still on hold ("HELD") or was
        /// bounced ("BOUNCED").
        held: bool,
        quote: StoredQuote,
        identity_mode: &'static str,
    },
}

pub(crate) async fn get(State(state): State<AppState>, Path(token): Path<String>) -> RouteResult {
    let db = state.db().await?;
    let Some(challenge) = challenge_by_token(db, &token).await? else {
        return Err(refuse(StatusCode::NOT_FOUND, "Unknown challenge"));
    };

    // Truthiness, as the page tested it.
    if challenge
        .resolved_at
        .is_some_and(|resolved_at| resolved_at != 0)
    {
        return Ok(json(StatusCode::OK, &ChallengeView::Resolved));
    }

    let Some(quote) = parse_stored_quote(&challenge.quote_json) else {
        return Ok(json(StatusCode::OK, &ChallengeView::Dead));
    };

    // Read only for an open challenge, where the page read it, so a
    // misconfigured mode fails exactly the renders it failed before.
    let identity_mode = match identity_mode(state.env().lookup())? {
        IdentityMode::Live => "live",
        IdentityMode::Mock => "mock",
    };

    Ok(json(
        StatusCode::OK,
        &ChallengeView::Open {
            dangerous: challenge.tier == DANGEROUS,
            held: challenge.held_until.is_some(),
            handle: challenge.handle,
            quote,
            identity_mode,
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::config::Env;
    use crate::db::challenges::{NewChallenge, claim_challenge};
    use crate::db::testing::TestDb;
    use crate::routes::router;
    use crate::routes::testing::{
        Answer, NOW, challenge, clock_at, get as get_request, seed, send,
    };

    fn quote_json() -> serde_json::Value {
        json!({
            "messageId": format!("0x{}", "ab".repeat(32)),
            "inbox": format!("0x{}", "cd".repeat(20)),
            "tier": "commercial",
            "amount": "1000000",
            "expiresAt": 1_800_000_000u64,
            "signature": format!("0x{}", "ef".repeat(65)),
            "reasons": ["looked automated"],
        })
    }

    async fn db_with(challenge: NewChallenge) -> Arc<TestDb> {
        let db = Arc::new(TestDb::fresh().await);
        seed(&db, &challenge).await;
        db
    }

    fn open(token: &str, tier: &str) -> NewChallenge {
        NewChallenge {
            quote_json: quote_json().to_string(),
            ..challenge(token, tier)
        }
    }

    async fn view(db: &Arc<TestDb>, env: Env, token: &str) -> Answer {
        let app = router(
            AppState::builder(env)
                .clock(clock_at(NOW))
                .db(db.clone())
                .build(),
        );
        send(app, get_request(&format!("/api/challenge/{token}"))).await
    }

    #[tokio::test]
    async fn an_unknown_token_is_not_found() {
        let db = db_with(open("tok", "commercial")).await;

        let answer = view(&db, Env::empty(), "bogus").await;

        assert_eq!(answer.status, StatusCode::NOT_FOUND);
        assert_eq!(answer.body, json!({ "error": "Unknown challenge" }));
    }

    #[tokio::test]
    async fn an_open_challenge_carries_what_the_page_rendered_and_nothing_else() {
        let db = db_with(open("tok", "commercial")).await;

        let answer = view(&db, Env::fixed([("IDENTITY_MODE", "mock")]), "tok").await;

        assert_eq!(answer.status, StatusCode::OK);
        assert_eq!(
            answer.body,
            json!({
                "state": "open",
                "handle": "demo",
                "dangerous": false,
                "held": true,
                "quote": quote_json(),
                "identityMode": "mock",
            })
        );
    }

    /// The sender's address and the message id are on the row but were never
    /// on the page.
    #[tokio::test]
    async fn the_sender_and_message_id_are_not_disclosed() {
        let db = db_with(open("tok", "commercial")).await;

        let answer = view(&db, Env::empty(), "tok").await;

        assert!(!answer.text.contains("sender@x.com"), "{}", answer.text);
        assert_eq!(answer.body["identityMode"], "live");
    }

    #[tokio::test]
    async fn a_settled_challenge_says_only_that_it_is_done() {
        let db = db_with(open("tok", "commercial")).await;
        claim_challenge(&db, "tok", "human", NOW).await.unwrap();

        let answer = view(&db, Env::empty(), "tok").await;

        assert_eq!(answer.text, r#"{"state":"resolved"}"#);
    }

    #[tokio::test]
    async fn a_challenge_whose_quote_does_not_parse_is_a_dead_link() {
        let db = db_with(challenge("tok", "commercial")).await;

        let answer = view(&db, Env::empty(), "tok").await;

        assert_eq!(answer.text, r#"{"state":"dead"}"#);
    }

    #[tokio::test]
    async fn dangerous_and_bounced_mail_is_flagged_as_the_page_flagged_it() {
        let db = db_with(NewChallenge {
            held_until: None,
            ..open("tok", "dangerous")
        })
        .await;

        let answer = view(&db, Env::empty(), "tok").await;

        assert_eq!(answer.body["dangerous"], true);
        assert_eq!(answer.body["held"], false);
    }

    /// The page called `identityMode()`, which throws on a near-miss; that was
    /// a failed render, and here it is the same bare 500.
    #[tokio::test]
    async fn an_unrecognised_identity_mode_fails_the_open_view_bare() {
        let db = db_with(open("tok", "commercial")).await;

        let answer = view(&db, Env::fixed([("IDENTITY_MODE", "Mock ")]), "tok").await;

        assert_eq!(answer.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(answer.text, "");
    }
}
