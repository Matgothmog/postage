//! Cloudflare Email Routing destination addresses (`web/src/lib/cloudflare.ts`).
//!
//! A destination has to be registered on the account before the worker's
//! `message.forward()` will accept it, and registering one is what makes
//! Cloudflare mail its owner a confirmation link.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::config::{ConfigError, required};

pub const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";

/// How long one Cloudflare API call may take. The TypeScript set none.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const PER_PAGE: usize = 50;

/// A ceiling on the walk, not on the account. Reaching it means the address is
/// not registered as far as anyone can tell, which is what the caller does
/// with a miss anyway; an unbounded loop against an API that kept answering
/// with a full page would be worse than a wrong answer.
const MAX_PAGES: usize = 40;

/// A destination address on the account. `verified_at` is `None` until the
/// owner clicks the link Cloudflare mails them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub id: String,
    pub verified_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudflareError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("{0}")]
    Transport(String),
    /// An answer that is not JSON at all: an edge 5xx or a WAF challenge
    /// answers with HTML.
    #[error("Cloudflare returned {0}")]
    Unreadable(u16),
    /// A refusal, worded as Cloudflare worded it.
    #[error("{0}")]
    Refused(String),
    /// JSON, but not the shape an address has.
    #[error("Cloudflare response was not an address: {0}")]
    Decode(String),
}

#[derive(Clone)]
pub struct Cloudflare {
    client: reqwest::Client,
    account_id: String,
    api_token: String,
    base: String,
}

impl fmt::Debug for Cloudflare {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Cloudflare")
            .field("account_id", &self.account_id)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// What every response is wrapped in. Read loosely: a refusal's body is not
/// guaranteed to carry every field, and a missing one must not turn a clear
/// refusal into a parse error.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Envelope {
    success: bool,
    errors: Vec<ApiError>,
    result: Value,
    /// Present on list responses only.
    result_info: Option<ResultInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ApiError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResultInfo {
    total_count: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct Address {
    id: String,
    email: String,
    verified: Option<String>,
}

impl Envelope {
    fn first_error(&self) -> Option<&str> {
        self.errors
            .first()
            .and_then(|error| error.message.as_deref())
    }

    /// The one refusal that carries information rather than a fault: this
    /// address is already on the account. ASCII case is ignored, as `/i` does
    /// without the `u` flag.
    fn is_duplicate(&self) -> bool {
        self.errors.iter().any(|error| {
            let message = error
                .message
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            message.contains("exist") || message.contains("already")
        })
    }

    fn result_as<T: DeserializeOwned>(&self) -> Result<T, CloudflareError> {
        T::deserialize(&self.result).map_err(|error| CloudflareError::Decode(error.to_string()))
    }
}

impl Cloudflare {
    pub fn new(client: reqwest::Client, account_id: String, api_token: String) -> Self {
        Self {
            client,
            account_id,
            api_token,
            base: DEFAULT_API_BASE.to_owned(),
        }
    }

    /// Reads `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`.
    pub fn from_env<F>(env: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Ok(Self::new(
            reqwest::Client::default(),
            required(&env, "CLOUDFLARE_ACCOUNT_ID")?,
            required(&env, "CLOUDFLARE_API_TOKEN")?,
        ))
    }

    /// Points the client somewhere other than Cloudflare's API.
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Registers the address so `message.forward()` will accept it, which also
    /// makes Cloudflare send its verification mail. Addresses are shared across
    /// the whole account, so one another user already registered comes back as
    /// it stands rather than as an error.
    pub async fn ensure_destination(&self, email: &str) -> Result<Destination, CloudflareError> {
        let body = json!({ "email": email.to_lowercase() });
        let created = self
            .call(
                reqwest::Method::POST,
                "/email/routing/addresses",
                Some(body),
            )
            .await?;
        if created.success {
            return to_destination(created.result_as()?);
        }

        if let Some(existing) = self.find_destination(email).await? {
            return Ok(existing);
        }

        Err(CloudflareError::Refused(
            created
                .first_error()
                .unwrap_or("Cloudflare refused the destination address")
                .to_owned(),
        ))
    }

    /// The address's current state, or `None` when Cloudflare answers without
    /// success but also without a fault (a duplicate-shaped refusal).
    pub async fn destination_status(
        &self,
        id: &str,
    ) -> Result<Option<Destination>, CloudflareError> {
        let found = self
            .call(
                reqwest::Method::GET,
                &format!("/email/routing/addresses/{id}"),
                None,
            )
            .await?;
        if !found.success {
            return Ok(None);
        }
        to_destination(found.result_as()?).map(Some)
    }

    /// Every page, not the first one. One destination is created per inbox and
    /// none are ever deleted, so a single page stopped finding older addresses
    /// at roughly the fiftieth signup, and a miss reads as "Cloudflare refused
    /// the address": a signup that fails for good.
    async fn find_destination(&self, email: &str) -> Result<Option<Destination>, CloudflareError> {
        let wanted = email.to_lowercase();

        for page in 1..=MAX_PAGES {
            let path =
                format!("/email/routing/addresses?per_page={PER_PAGE}&page={page}&direction=desc");
            let listed = self.call(reqwest::Method::GET, &path, None).await?;
            let addresses: Vec<Address> = if listed.result.is_null() {
                Vec::new()
            } else {
                listed.result_as()?
            };

            let length = addresses.len();
            if let Some(found) = addresses
                .into_iter()
                .find(|address| address.email.to_lowercase() == wanted)
            {
                return to_destination(found).map(Some);
            }

            // A short page is the end of the list whatever else is said. The
            // count only ever stops the walk early, so a response without it
            // reads as "keep going".
            if length < PER_PAGE {
                return Ok(None);
            }
            let total = listed.result_info.and_then(|info| info.total_count);
            if total.is_some_and(|total| page * PER_PAGE >= total) {
                return Ok(None);
            }
        }

        Ok(None)
    }

    /// One API call. A refusal naming an address as already registered is an
    /// answer we act on; anything else that is not a success status (a revoked
    /// token, a rate limit, an outage) is a failure, because returning it as
    /// data would read as "not verified yet" forever.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Envelope, CloudflareError> {
        let url = format!("{}/accounts/{}{path}", self.base, self.account_id);
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&self.api_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(REQUEST_TIMEOUT);
        if let Some(body) = body {
            request = request.body(body.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|error| CloudflareError::Transport(error.without_url().to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| CloudflareError::Transport(error.without_url().to_string()))?;

        let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
            return Err(CloudflareError::Unreadable(status.as_u16()));
        };
        if !status.is_success() && !envelope.is_duplicate() {
            let message = envelope.first_error().map_or_else(
                || format!("Cloudflare returned {}", status.as_u16()),
                str::to_owned,
            );
            return Err(CloudflareError::Refused(message));
        }
        Ok(envelope)
    }
}

/// Cloudflare sends a zero date rather than null for an address it has not
/// verified, and an unparseable date must not become a timestamp at all.
fn to_destination(address: Address) -> Result<Destination, CloudflareError> {
    let verified_at = address
        .verified
        .as_deref()
        .filter(|verified| !verified.is_empty())
        .and_then(parse_js_date_millis)
        .map(|millis| millis.div_euclid(1000))
        .filter(|seconds| *seconds > 0);
    Ok(Destination {
        id: address.id,
        verified_at,
    })
}

/// JavaScript's `Date.parse` for the ISO 8601 forms it reads, in epoch
/// milliseconds: `YYYY`, `YYYY-MM`, `YYYY-MM-DD` (as UTC), each optionally
/// followed by `T`, `HH:mm`, `:ss`, a fraction of any length (truncated to
/// milliseconds) and `Z` or a `±HH:mm` offset. Like V8 it also takes a space
/// or a lowercase `t`/`z`, an offset without its colon, the hour 24 at
/// exactly midnight, and a day past the end of its month (rolled over).
///
/// A date-time without an offset is read as UTC, where JavaScript would use
/// the host's zone; the functions this ran on are UTC. V8's legacy fallback
/// for other formats ("Jan 1 2026") is not reproduced: those read as `None`.
fn parse_js_date_millis(text: &str) -> Option<i64> {
    let mut cursor = Cursor { rest: text };
    let year = cursor.year()?;
    let mut month = 1;
    let mut day = 1;
    if cursor.eat('-') {
        month = cursor.digits(2)?;
        if cursor.eat('-') {
            day = cursor.digits(2)?;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut millis_of_day = 0;
    let mut offset_minutes = 0;
    if cursor.eat_any(&['T', 't', ' ']) {
        let hour = cursor.digits(2)?;
        cursor.expect(':')?;
        let minute = cursor.digits(2)?;
        let second = if cursor.eat(':') {
            cursor.digits(2)?
        } else {
            0
        };
        let millis = if cursor.eat('.') {
            cursor.fraction_millis()?
        } else {
            0
        };
        let past_midnight = hour == 24 && minute == 0 && second == 0 && millis == 0;
        if (hour > 23 && !past_midnight) || minute > 59 || second > 59 {
            return None;
        }
        millis_of_day = ((hour * 60 + minute) * 60 + second) * 1000 + millis;
        offset_minutes = cursor.offset()?;
    }
    if !cursor.rest.is_empty() {
        return None;
    }

    let days = days_from_civil(year, month, 1) + day - 1;
    Some(days * 86_400_000 + millis_of_day - offset_minutes * 60_000)
}

struct Cursor<'a> {
    rest: &'a str,
}

impl Cursor<'_> {
    fn eat(&mut self, expected: char) -> bool {
        self.eat_any(&[expected])
    }

    fn eat_any(&mut self, expected: &[char]) -> bool {
        match self.rest.strip_prefix(expected) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn expect(&mut self, expected: char) -> Option<()> {
        self.eat(expected).then_some(())
    }

    fn digits(&mut self, count: usize) -> Option<i64> {
        let digits = self.rest.get(..count)?;
        if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        self.rest = &self.rest[count..];
        digits.parse().ok()
    }

    /// Four digits, or a sign and six; the expanded year zero cannot be
    /// negative.
    fn year(&mut self) -> Option<i64> {
        if self.eat('+') {
            return self.digits(6);
        }
        if self.eat('-') {
            let year = self.digits(6)?;
            return (year != 0).then_some(-year);
        }
        self.digits(4)
    }

    /// At least one digit; the first three are milliseconds.
    fn fraction_millis(&mut self) -> Option<i64> {
        let length = self.rest.bytes().take_while(u8::is_ascii_digit).count();
        if length == 0 {
            return None;
        }
        let digits = &self.rest[..length];
        self.rest = &self.rest[length..];
        let padded = format!("{digits:0<3}");
        padded[..3].parse().ok()
    }

    /// Minutes east of UTC: `Z`, `±HH:mm`, `±HHmm`, or nothing (UTC).
    fn offset(&mut self) -> Option<i64> {
        if self.eat_any(&['Z', 'z']) || self.rest.is_empty() {
            return Some(0);
        }
        let sign = if self.eat('+') {
            1
        } else if self.eat('-') {
            -1
        } else {
            return None;
        };
        let hours = self.digits(2)?;
        self.eat(':');
        let minutes = self.digits(2)?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        Some(sign * (hours * 60 + minutes))
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;
    use crate::http_stub::{Reply, Sent, Stub, closed_port, serve_with};

    fn address(id: &str, email: &str, verified: Option<&str>) -> Value {
        json!({ "id": id, "email": email, "verified": verified })
    }

    /// A full page of addresses that are not the one wanted, with an optional
    /// match planted at an index, so the walk is shown to look at every row.
    fn full_page(page: usize, planted: Option<(&str, usize)>) -> Vec<Value> {
        (0..PER_PAGE)
            .map(|index| match planted {
                Some((email, at)) if at == index => {
                    address(&format!("p{page}-{index}"), email, None)
                }
                _ => address(
                    &format!("p{page}-{index}"),
                    &format!("nobody-{page}-{index}@example.com"),
                    None,
                ),
            })
            .collect()
    }

    fn ok(result: Value) -> Reply {
        Reply::new(
            200,
            json!({ "success": true, "errors": [], "result": result }).to_string(),
        )
    }

    fn listing(page: usize, addresses: Vec<Value>, total_count: usize) -> Reply {
        Reply::new(
            200,
            json!({
                "success": true,
                "errors": [],
                "result": addresses,
                "result_info": { "page": page, "per_page": PER_PAGE, "total_count": total_count },
            })
            .to_string(),
        )
    }

    fn client(stub: &Stub) -> Cloudflare {
        Cloudflare::new(
            reqwest::Client::default(),
            "test-account".to_owned(),
            "test-token".to_owned(),
        )
        .with_api_base(&stub.base)
    }

    fn page_of(sent: &Sent) -> usize {
        sent.query_param("page").unwrap().parse().unwrap()
    }

    /// The listing is reached only when the create call is refused as a
    /// duplicate, which is the one door the module gives it.
    async fn serve_duplicate<F>(list_page: F) -> Stub
    where
        F: Fn(&Sent) -> Reply + Send + Sync + 'static,
    {
        serve_with(move |sent| {
            if sent.method == "POST" {
                return Reply::new(
                    409,
                    json!({
                        "success": false,
                        "errors": [{ "code": 1, "message": "address already exists" }],
                        "result": null,
                    })
                    .to_string(),
                );
            }
            list_page(sent)
        })
        .await
    }

    #[tokio::test]
    async fn a_duplicate_address_findable_on_the_first_page_is_found_without_walking_further() {
        let stub = serve_duplicate(|sent| {
            assert_eq!(page_of(sent), 1, "must start the walk on page one");
            listing(1, full_page(1, Some(("found@example.com", 10))), 200)
        })
        .await;

        let found = client(&stub)
            .ensure_destination("found@example.com")
            .await
            .unwrap();

        assert_eq!(found.id, "p1-10");
        let listed = stub
            .sent()
            .iter()
            .filter(|sent| sent.method == "GET")
            .count();
        assert_eq!(
            listed, 1,
            "a first-page match must not trigger a second request"
        );
    }

    /// The regression the module's comment names: reading only page one
    /// stopped finding addresses at roughly the fiftieth signup.
    #[tokio::test]
    async fn a_duplicate_address_on_a_later_page_is_found_by_walking_past_page_one() {
        let wanted = "later-signup@example.com";
        let stub = serve_duplicate(move |sent| {
            let page = page_of(sent);
            assert!(page <= 3, "must stop once page {} held the match", page - 1);
            let planted = (page == 3).then_some((wanted, 5));
            listing(page, full_page(page, planted), 200)
        })
        .await;

        let found = client(&stub).ensure_destination(wanted).await.unwrap();

        assert_eq!(
            found.id, "p3-5",
            "a match on page three of five must be found"
        );
    }

    #[tokio::test]
    async fn a_duplicate_address_that_cannot_be_found_anywhere_fails_instead_of_hanging() {
        const TOTAL_PAGES: usize = 4;
        let stub = serve_duplicate(|sent| {
            let page = page_of(sent);
            let mut addresses = full_page(page, None);
            if page == TOTAL_PAGES {
                addresses.truncate(10);
            }
            listing(page, addresses, (TOTAL_PAGES - 1) * PER_PAGE + 10)
        })
        .await;

        let error = client(&stub)
            .ensure_destination("nobody-registered@example.com")
            .await
            .unwrap_err();

        assert_eq!(
            error,
            CloudflareError::Refused("address already exists".to_owned())
        );
        let listed = stub
            .sent()
            .iter()
            .filter(|sent| sent.method == "GET")
            .count();
        assert_eq!(
            listed, TOTAL_PAGES,
            "the walk must cover every page the account reports"
        );
    }

    #[tokio::test]
    async fn a_zero_date_verified_field_becomes_none_rather_than_an_unparseable_date() {
        for verified in [None, Some("0001-01-01T00:00:00Z")] {
            let stub = serve_with(move |_| ok(address("addr-1", "x@example.com", verified))).await;

            let status = client(&stub).destination_status("addr-1").await.unwrap();

            assert_eq!(status.unwrap().verified_at, None, "{verified:?}");
        }
    }

    #[tokio::test]
    async fn a_real_verified_timestamp_survives_as_seconds_since_epoch() {
        let stub = serve_with(|_| {
            ok(address(
                "addr-2",
                "x@example.com",
                Some("2026-01-01T00:00:00Z"),
            ))
        })
        .await;

        let status = client(&stub).destination_status("addr-2").await.unwrap();

        assert_eq!(status.unwrap().verified_at, Some(1_767_225_600));
    }

    #[tokio::test]
    async fn an_unparseable_verified_value_never_becomes_a_timestamp() {
        let stub = serve_with(|_| ok(address("addr-3", "x@example.com", Some("not-a-date")))).await;

        let status = client(&stub).destination_status("addr-3").await.unwrap();

        assert_eq!(status.unwrap().verified_at, None);
    }

    #[tokio::test]
    async fn ensure_destination_on_a_fresh_address_uses_the_create_response_directly() {
        let stub = serve_with(|sent| {
            assert!(sent.path.ends_with("/addresses"));
            ok(address("new-addr", "fresh@example.com", None))
        })
        .await;

        let created = client(&stub)
            .ensure_destination("Fresh@Example.com")
            .await
            .unwrap();

        assert_eq!(created.id, "new-addr");
        let sent = stub.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method, "POST");
        assert_eq!(
            sent[0].path,
            "/accounts/test-account/email/routing/addresses"
        );
        assert_eq!(sent[0].body, json!({ "email": "fresh@example.com" }));
        assert_eq!(sent[0].header("authorization"), Some("Bearer test-token"));
        assert_eq!(sent[0].content_type.as_deref(), Some("application/json"));
    }

    /// An outage or a revoked token must surface as a failure, not as data.
    #[tokio::test]
    async fn a_non_duplicate_refusal_is_never_read_as_not_verified_yet() {
        let stub = serve_with(|_| {
            Reply::new(
                500,
                json!({ "success": false, "errors": [{ "code": 999, "message": "internal error" }], "result": null })
                    .to_string(),
            )
        })
        .await;

        let error = client(&stub)
            .destination_status("addr-x")
            .await
            .unwrap_err();

        assert_eq!(error, CloudflareError::Refused("internal error".to_owned()));
    }

    #[tokio::test]
    async fn an_html_error_page_is_a_failure_naming_the_status() {
        let stub = serve_with(|_| Reply::new(502, "<html>bad gateway</html>")).await;

        let error = client(&stub)
            .destination_status("addr-x")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "Cloudflare returned 502");
    }

    #[tokio::test]
    async fn a_refusal_without_a_message_names_the_status() {
        let stub = serve_with(|_| Reply::new(403, r#"{"success":false,"errors":[]}"#)).await;

        let error = client(&stub)
            .destination_status("addr-x")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "Cloudflare returned 403");
    }

    /// A duplicate-shaped refusal on a lookup is not a fault, but it is not an
    /// address either.
    #[tokio::test]
    async fn a_status_lookup_that_is_answered_without_success_is_none() {
        let stub = serve_with(|_| {
            Reply::new(
                409,
                r#"{"success":false,"errors":[{"message":"Already there"}]}"#,
            )
        })
        .await;

        assert_eq!(
            client(&stub).destination_status("addr-x").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn an_unreachable_api_is_a_transport_failure() {
        let cloudflare = Cloudflare::new(
            reqwest::Client::default(),
            "test-account".to_owned(),
            "test-token".to_owned(),
        )
        .with_api_base(closed_port());

        let error = cloudflare.destination_status("addr-x").await.unwrap_err();

        assert!(matches!(error, CloudflareError::Transport(_)), "{error}");
        assert!(!error.to_string().contains("test-token"));
    }

    /// A list that never ends is walked to the ceiling and no further.
    #[tokio::test]
    async fn the_walk_stops_at_its_ceiling_when_every_page_is_full_and_uncounted() {
        let pages = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&pages);
        let stub = serve_duplicate(move |sent| {
            counter.fetch_add(1, Ordering::SeqCst);
            Reply::new(
                200,
                json!({ "success": true, "errors": [], "result": full_page(page_of(sent), None) })
                    .to_string(),
            )
        })
        .await;

        let error = client(&stub)
            .ensure_destination("missing@example.com")
            .await;

        assert!(error.is_err());
        assert_eq!(pages.load(Ordering::SeqCst), MAX_PAGES);
    }

    #[test]
    fn from_env_needs_the_account_and_the_token() {
        let only_account =
            |name: &str| (name == "CLOUDFLARE_ACCOUNT_ID").then(|| "acct".to_owned());
        assert_eq!(
            Cloudflare::from_env(only_account).unwrap_err(),
            ConfigError::Missing("CLOUDFLARE_API_TOKEN")
        );
        assert!(
            !format!(
                "{:?}",
                Cloudflare::from_env(|_| Some("x".to_owned())).unwrap()
            )
            .contains("api_token")
        );
    }

    /// Each case's expected value is Node's `Date.parse` of the same text.
    #[test]
    fn dates_parse_as_javascript_parses_them() {
        let cases: [(&str, Option<i64>); 25] = [
            ("2026-01-01T00:00:00Z", Some(1_767_225_600_000)),
            ("0001-01-01T00:00:00Z", Some(-62_135_596_800_000)),
            ("2026-01-01", Some(1_767_225_600_000)),
            ("2026-01", Some(1_767_225_600_000)),
            ("2026", Some(1_767_225_600_000)),
            ("2026-01-01T00:00Z", Some(1_767_225_600_000)),
            ("2026-01-01T00:00:00.123456Z", Some(1_767_225_600_123)),
            ("2026-01-01T00:00:00.1Z", Some(1_767_225_600_100)),
            ("2026-02-30T00:00:00Z", Some(1_772_409_600_000)),
            ("2024-02-29T00:00:00Z", Some(1_709_164_800_000)),
            ("2026-01-01T24:00:00Z", Some(1_767_312_000_000)),
            ("2026-01-01T24:00:01Z", None),
            ("2026-01-01T00:00:00+02:00", Some(1_767_218_400_000)),
            ("2026-01-01T00:00:00-0230", Some(1_767_234_600_000)),
            ("+002026-01-01T00:00:00Z", Some(1_767_225_600_000)),
            ("-000001-01-01T00:00:00Z", Some(-62_198_755_200_000)),
            ("-000000-01-01T00:00:00Z", None),
            ("2026-13-01T00:00:00Z", None),
            ("2026-01-32T00:00:00Z", None),
            ("2026-01-01t00:00:00z", Some(1_767_225_600_000)),
            ("2026-01-01 00:00:00Z", Some(1_767_225_600_000)),
            ("not-a-date", None),
            ("2026-01-01T00:00:60Z", None),
            ("2026-01-01T00:00:00.Z", None),
            ("2026-01-01T00:00:00+24:00", None),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_js_date_millis(text), expected, "{text}");
        }
    }

    #[test]
    fn the_epoch_and_its_first_second_parse_to_their_milliseconds() {
        assert_eq!(parse_js_date_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_js_date_millis("1970-01-01T00:00:01.999Z"), Some(1999));
        assert_eq!(
            parse_js_date_millis("2026-01-01T00:00:00+23:59"),
            Some(1_767_139_260_000)
        );
    }
}
