//! Releasing a held message (`web/src/lib/hold.ts`).
//!
//! The message never passes through here. The mail worker holds the bytes and
//! hands them straight to the relay, so this says "send it" and learns whether
//! it went, which is what keeps a released message identical to the one sent.

use std::fmt;
use std::time::Duration;

use postage_shared::{RELEASE_PATH, RELEASE_SECRET_HEADER, ReleaseRequest};

use crate::config::{ConfigError, required};
use crate::db::challenges::{claim_hold, mark_delivered};
use crate::db::inboxes::inbox_by_handle;
use crate::db::{Db, DbError};

/// How long the worker may take to say whether it sent the message. The
/// TypeScript set none; the request is bounded so a hung worker cannot hold a
/// sender's request open until the host kills the function. Generous on
/// purpose: a slow release that did go out but is reported as `SendFailed`
/// gives the sender a free paid use, and 25s still sits well under the host's
/// function limit.
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(25);

/// Whether the held message went out, and if not, which of three different
/// bugs to chase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    Delivered,
    Undelivered(Undelivered),
}

impl Release {
    pub fn delivered(self) -> bool {
        self == Self::Delivered
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undelivered {
    /// The hold was already released, or ran out.
    Expired,
    /// The handle has no inbox to deliver to.
    NoInbox,
    /// The worker could not be reached or refused.
    SendFailed,
}

impl Undelivered {
    /// The wire and log spelling, `reason_undelivered` in the TypeScript.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::NoInbox => "no_inbox",
            Self::SendFailed => "send_failed",
        }
    }
}

impl fmt::Display for Undelivered {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The mail worker's release endpoint and the secret it answers to.
#[derive(Clone)]
pub struct MailWorker {
    client: reqwest::Client,
    url: String,
    secret: String,
    timeout: Duration,
    /// Set when the worker's settings were missing when it was built: every
    /// release then fails as the TypeScript's did, by `required()` throwing
    /// inside the send and the gate carrying on without the message.
    missing: Option<ConfigError>,
}

impl fmt::Debug for MailWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MailWorker")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl MailWorker {
    /// `base_url` is the worker's origin; the release path is appended.
    pub fn new(client: reqwest::Client, base_url: &str, secret: impl Into<String>) -> Self {
        Self {
            client,
            url: format!("{base_url}{RELEASE_PATH}"),
            secret: secret.into(),
            timeout: RELEASE_TIMEOUT,
            missing: None,
        }
    }

    /// Shortens the release timeout so tests need not wait out twenty-five seconds.
    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Reads `MAIL_WORKER_URL` and `MAIL_WEBHOOK_SECRET`.
    pub fn from_env<F>(env: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let url = required(&env, "MAIL_WORKER_URL")?;
        let secret = required(&env, "MAIL_WEBHOOK_SECRET")?;
        Ok(Self::new(reqwest::Client::default(), &url, secret))
    }

    /// [`MailWorker::from_env`], except that missing settings leave a worker
    /// whose every send fails rather than no worker at all. The TypeScript read
    /// them inside the send, so an unconfigured worker cost a delivery, not
    /// the request that opened the gate.
    pub fn from_env_or_unconfigured<F>(env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        Self::from_env(env).unwrap_or_else(|missing| Self {
            missing: Some(missing),
            ..Self::new(reqwest::Client::default(), "", "")
        })
    }

    /// Sends the message held under `token` to `handle`'s inbox.
    ///
    /// The hold is claimed first, and claiming is one conditional update: two
    /// clicks a second apart cannot both come away believing they may send
    /// it, so a message cannot be delivered twice. A failure afterwards leaves
    /// the sender with the paste-it-back route rather than a duplicate in
    /// someone's inbox. Only reading the inbox and claiming the hold can fail
    /// this call; everything after them is reported as `Undelivered`.
    pub async fn release_held_message(
        &self,
        db: &Db,
        token: &str,
        handle: &str,
        now: i64,
    ) -> Result<Release, DbError> {
        let Some(inbox) = inbox_by_handle(db, handle).await? else {
            return Ok(Release::Undelivered(Undelivered::NoInbox));
        };
        if !claim_hold(db, token, now).await? {
            return Ok(Release::Undelivered(Undelivered::Expired));
        }

        if !self.send(token, &inbox.destination).await {
            return Ok(Release::Undelivered(Undelivered::SendFailed));
        }
        // The worker has already sent it. If recording that fails the delivery
        // still happened, and letting the error escape would reopen the
        // challenge and hand out a second pass for the same payment.
        let _ = mark_delivered(db, token, now).await;
        Ok(Release::Delivered)
    }

    /// Whether the worker answered 2xx. The body is not read: the TypeScript
    /// took any success status as sent.
    async fn send(&self, token: &str, destination: &str) -> bool {
        if self.missing.is_some() {
            return false;
        }
        let request = ReleaseRequest {
            token: token.to_owned(),
            to: destination.to_owned(),
        };
        let Ok(body) = serde_json::to_string(&request) else {
            return false;
        };
        self.client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(RELEASE_SECRET_HEADER, &self.secret)
            .body(body)
            .timeout(self.timeout)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::db::challenges::{NewChallenge, challenge_by_token, create_challenge};
    use crate::db::inboxes::create_inbox;
    use crate::db::testing::TestDb;
    use crate::http_stub::{Reply, Stub, closed_port, serve, serve_with};

    const NOW: i64 = 1_788_868_800;
    const HANDLE: &str = "demo";
    const SENDER: &str = "sender@x.com";
    const SECRET: &str = "test";

    /// Older than any clock in these tests, so a second write of
    /// `delivered_at` is unmistakable rather than a same-second coincidence.
    const AN_EARLIER_DELIVERY: i64 = 1_700_000_000;

    async fn with_inbox() -> TestDb {
        let db = TestDb::fresh().await;
        create_inbox(
            &db,
            HANDLE,
            "demo@example.com",
            Some(&format!("0x{}", "11".repeat(20))),
            NOW,
        )
        .await
        .unwrap();
        db
    }

    async fn held(db: &Db, token: &str) {
        create_challenge(
            db,
            &NewChallenge {
                token: token.to_owned(),
                handle: HANDLE.to_owned(),
                sender: SENDER.to_owned(),
                message_id: format!("0x{}", "ab".repeat(32)),
                tier: "commercial".to_owned(),
                amount: "1".to_owned(),
                held_until: Some(NOW + 900),
                quote_json: "{}".to_owned(),
                created_at: NOW,
            },
        )
        .await
        .unwrap();
    }

    fn worker(stub: &Stub) -> MailWorker {
        MailWorker::new(reqwest::Client::default(), &stub.base, SECRET)
    }

    /// A worker that sends whatever it is asked to and counts the requests,
    /// which is how a second release is seen from outside.
    async fn counting_worker() -> (Stub, Arc<AtomicUsize>) {
        let releases = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&releases);
        let stub = serve_with(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::new(200, r#"{"sent":true}"#)
        })
        .await;
        (stub, releases)
    }

    /// The guard `mark_delivered` does without lives one step upstream:
    /// recording a delivery is only reached behind `claim_hold`, which nothing
    /// turns back. Take that away and the timestamp of the delivery that
    /// happened is overwritten by one that did not.
    #[tokio::test]
    async fn a_held_message_releases_once_so_the_moment_it_was_delivered_is_written_once() {
        let db = with_inbox().await;
        held(&db, "once").await;
        let (stub, releases) = counting_worker().await;
        let worker = worker(&stub);

        assert_eq!(
            worker
                .release_held_message(&db, "once", HANDLE, NOW)
                .await
                .unwrap(),
            Release::Delivered
        );
        let delivered = challenge_by_token(&db, "once").await.unwrap().unwrap();
        assert!(delivered.delivered_at.is_some());

        db.run(
            "UPDATE challenges SET delivered_at = ? WHERE token = ?",
            libsql::params![AN_EARLIER_DELIVERY, "once"],
        )
        .await
        .unwrap();

        assert_eq!(
            worker
                .release_held_message(&db, "once", HANDLE, NOW)
                .await
                .unwrap(),
            Release::Undelivered(Undelivered::Expired)
        );
        assert_eq!(
            releases.load(Ordering::SeqCst),
            1,
            "the hold was already spent, so the worker must not be asked again"
        );
        let after = challenge_by_token(&db, "once").await.unwrap().unwrap();
        assert_eq!(after.delivered_at, Some(AN_EARLIER_DELIVERY));
    }

    /// Sequentially the loser sees a hold already cleared; at once, both read
    /// a live one and the database decides.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_releases_racing_on_one_token_send_it_once() {
        let db = with_inbox().await;
        held(&db, "race").await;
        let (stub, releases) = counting_worker().await;
        let worker = worker(&stub);

        let (first, second) = tokio::join!(
            worker.release_held_message(&db, "race", HANDLE, NOW),
            worker.release_held_message(&db, "race", HANDLE, NOW)
        );
        let outcomes = [first.unwrap(), second.unwrap()];

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| outcome.delivered())
                .count(),
            1
        );
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        let stamped = challenge_by_token(&db, "race").await.unwrap().unwrap();
        assert!(stamped.delivered_at.is_some());
    }

    #[tokio::test]
    async fn the_worker_is_asked_with_the_secret_the_token_and_the_inbox_destination() {
        let db = with_inbox().await;
        held(&db, "asked").await;
        let stub = serve(&[(RELEASE_PATH, 200, r#"{"sent":true}"#)]).await;

        worker(&stub)
            .release_held_message(&db, "asked", HANDLE, NOW)
            .await
            .unwrap();

        let sent = stub.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "POST");
        assert_eq!(sent[0].path, "/release");
        assert_eq!(sent[0].header(RELEASE_SECRET_HEADER), Some(SECRET));
        assert_eq!(sent[0].content_type.as_deref(), Some("application/json"));
        assert_eq!(
            sent[0].body,
            json!({ "token": "asked", "to": "demo@example.com" })
        );
    }

    #[tokio::test]
    async fn a_handle_with_no_inbox_is_not_released_and_keeps_its_hold() {
        let db = TestDb::fresh().await;
        held(&db, "orphan").await;
        let (stub, releases) = counting_worker().await;

        let release = worker(&stub)
            .release_held_message(&db, "orphan", HANDLE, NOW)
            .await
            .unwrap();

        assert_eq!(release, Release::Undelivered(Undelivered::NoInbox));
        assert_eq!(releases.load(Ordering::SeqCst), 0);
        let challenge = challenge_by_token(&db, "orphan").await.unwrap().unwrap();
        assert_eq!(challenge.held_until, Some(NOW + 900));
    }

    /// The hold is spent before the worker is asked, so a refusal leaves the
    /// sender with paste-it-back rather than a second attempt.
    #[tokio::test]
    async fn a_worker_that_refuses_is_a_failed_send_and_records_no_delivery() {
        let db = with_inbox().await;
        held(&db, "refused").await;
        let stub = serve(&[(RELEASE_PATH, 502, "relay down")]).await;

        let release = worker(&stub)
            .release_held_message(&db, "refused", HANDLE, NOW)
            .await
            .unwrap();

        assert_eq!(release, Release::Undelivered(Undelivered::SendFailed));
        let challenge = challenge_by_token(&db, "refused").await.unwrap().unwrap();
        assert_eq!(challenge.delivered_at, None);
        assert_eq!(challenge.held_until, None);
    }

    #[tokio::test]
    async fn a_worker_that_cannot_be_reached_is_a_failed_send() {
        let db = with_inbox().await;
        held(&db, "unreachable").await;
        let worker = MailWorker::new(reqwest::Client::default(), &closed_port(), SECRET);

        let release = worker
            .release_held_message(&db, "unreachable", HANDLE, NOW)
            .await
            .unwrap();

        assert_eq!(release, Release::Undelivered(Undelivered::SendFailed));
    }

    /// The TypeScript read any 2xx as sent and never parsed the body.
    #[tokio::test]
    async fn any_success_status_counts_as_sent_whatever_the_body() {
        let db = with_inbox().await;
        held(&db, "terse").await;
        let stub = serve(&[(RELEASE_PATH, 204, "")]).await;

        let release = worker(&stub)
            .release_held_message(&db, "terse", HANDLE, NOW)
            .await
            .unwrap();

        assert_eq!(release, Release::Delivered);
    }

    #[test]
    fn the_worker_gets_twenty_five_seconds_by_default() {
        assert_eq!(RELEASE_TIMEOUT, Duration::from_secs(25));
        let worker = MailWorker::new(reqwest::Client::default(), "https://w.test", SECRET);
        assert_eq!(worker.timeout, RELEASE_TIMEOUT);
    }

    #[tokio::test]
    async fn a_worker_that_answers_too_slowly_is_a_failed_send() {
        let db = with_inbox().await;
        held(&db, "slow").await;
        let stub = serve_with(|_| Reply::new(200, "{}").after(Duration::from_secs(30))).await;
        let worker = worker(&stub).with_timeout(Duration::from_millis(200));

        let release = worker
            .release_held_message(&db, "slow", HANDLE, NOW)
            .await
            .unwrap();

        assert_eq!(release, Release::Undelivered(Undelivered::SendFailed));
        let challenge = challenge_by_token(&db, "slow").await.unwrap().unwrap();
        assert_eq!(challenge.delivered_at, None);
    }

    #[test]
    fn the_reasons_are_spelled_as_the_typescript_logged_them() {
        assert_eq!(Undelivered::Expired.as_str(), "expired");
        assert_eq!(Undelivered::NoInbox.as_str(), "no_inbox");
        assert_eq!(Undelivered::SendFailed.as_str(), "send_failed");
    }

    #[test]
    fn from_env_needs_both_the_url_and_the_secret() {
        let only_url =
            |name: &str| (name == "MAIL_WORKER_URL").then(|| "https://w.test".to_owned());
        assert_eq!(
            MailWorker::from_env(only_url).unwrap_err(),
            ConfigError::Missing("MAIL_WEBHOOK_SECRET")
        );
        assert_eq!(
            MailWorker::from_env(|_| None).unwrap_err(),
            ConfigError::Missing("MAIL_WORKER_URL")
        );
    }

    /// The TypeScript read both settings inside the send, so a deployment
    /// without them still opened the gate and only lost the release.
    #[tokio::test]
    async fn an_unconfigured_worker_fails_the_send_but_not_the_release() {
        let db = with_inbox().await;
        held(&db, "unconfigured").await;
        let worker = MailWorker::from_env_or_unconfigured(|_| None);

        let release = worker
            .release_held_message(&db, "unconfigured", HANDLE, NOW)
            .await
            .unwrap();

        assert_eq!(release, Release::Undelivered(Undelivered::SendFailed));
    }

    #[test]
    fn the_secret_never_appears_in_debug_output() {
        let worker = MailWorker::new(reqwest::Client::default(), "https://w.test", "hunter2");
        let shown = format!("{worker:?}");
        assert!(shown.contains("https://w.test/release"));
        assert!(!shown.contains("hunter2"));
    }
}
