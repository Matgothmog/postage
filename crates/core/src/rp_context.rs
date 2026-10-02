use serde::{Deserialize, Deserializer};

/// How long a signed rp_context lives, and how long the browser may wait on
/// the World App under one.
///
/// The two belong in one file because they are one number in two units. The
/// SDK defaults disagree: `signRequest` signs a five minute window while
/// `pollUntilCompletion` waits fifteen, so a sender who took six minutes over
/// their selfie got `rp_signature_expired` back from World. Whichever way the
/// TTL moves, the polling window has to move with it.
///
/// Short on purpose: a signed context is a bearer credential, so how much a
/// stolen one is worth is capped by how briefly it lives.
pub const RP_CONTEXT_TTL_SECONDS: u64 = 300;

/// The gap between the browser giving up and the signature actually expiring.
/// It absorbs the round trip and any clock skew with World, so a slow sender
/// is told "that took too long" by their own poll loop rather than being
/// rejected by World.
pub const POLL_SAFETY_MARGIN_SECONDS: u64 = 30;

/// Floors the window so a very short TTL cannot produce a timeout a poll loop
/// would treat as already elapsed.
pub const MIN_POLL_SECONDS: u64 = 5;

/// The half of an `RpContext` that says when it was signed and when it stops
/// being worth anything. A missing, null or non-numeric timestamp reads as
/// `None`, and a numeric string reads as its number, as the browser hands them
/// over; either way `poll_timeout_ms` still returns a usable window.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct RpContextWindow {
    #[serde(default, deserialize_with = "lenient_seconds")]
    pub created_at: Option<f64>,
    #[serde(default, deserialize_with = "lenient_seconds")]
    pub expires_at: Option<f64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumberOrText {
    Number(f64),
    Text(String),
    Other(serde::de::IgnoredAny),
}

fn lenient_seconds<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    Ok(match NumberOrText::deserialize(deserializer)? {
        NumberOrText::Number(number) => Some(number),
        NumberOrText::Text(text) => match text.trim() {
            "" => Some(0.0),
            trimmed => trimmed.parse().ok(),
        },
        NumberOrText::Other(_) => None,
    })
}

impl RpContextWindow {
    pub const fn new(created_at: u64, expires_at: u64) -> Self {
        Self {
            created_at: Some(created_at as f64),
            expires_at: Some(expires_at as f64),
        }
    }
}

/// How long the client may wait for the World App under this context.
///
/// Read off the signature's own two timestamps rather than any wall clock, so
/// a browser whose clock is minutes out still polls for the window it was
/// actually granted.
pub fn poll_timeout_ms(context: &RpContextWindow) -> u64 {
    let granted = match (context.created_at, context.expires_at) {
        (Some(created), Some(expires)) => expires - created,
        _ => 0.0,
    };
    // A malformed window falls back to 0 so the floor below does its job for
    // it exactly as it does for a genuinely short or inverted one.
    let granted = if granted.is_finite() { granted } else { 0.0 };
    let seconds = (granted - POLL_SAFETY_MARGIN_SECONDS as f64).max(MIN_POLL_SECONDS as f64);
    (seconds * 1000.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CREATED_AT: u64 = 1_700_000_000;

    fn window_of(ttl_seconds: u64) -> RpContextWindow {
        RpContextWindow::new(CREATED_AT, CREATED_AT + ttl_seconds)
    }

    /// A runtime value that reached `poll_timeout_ms` without being checked
    /// first, as the browser can hand one over.
    fn malformed_window(value: serde_json::Value) -> RpContextWindow {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_poll_window_closes_before_the_signature_it_was_issued_under_expires() {
        let timeout = poll_timeout_ms(&window_of(RP_CONTEXT_TTL_SECONDS));

        assert_eq!(
            timeout,
            (RP_CONTEXT_TTL_SECONDS - POLL_SAFETY_MARGIN_SECONDS) * 1000
        );
        assert!(timeout < RP_CONTEXT_TTL_SECONDS * 1000);
    }

    #[test]
    fn measures_the_window_from_the_signatures_own_timestamps_not_any_wall_clock() {
        let decade_later = RpContextWindow::new(CREATED_AT + 315_360_000, CREATED_AT + 315_360_300);

        assert_eq!(
            poll_timeout_ms(&window_of(300)),
            poll_timeout_ms(&decade_later)
        );
    }

    #[test]
    fn never_hands_a_poll_loop_a_window_that_is_already_over() {
        let timeout = poll_timeout_ms(&window_of(POLL_SAFETY_MARGIN_SECONDS - 1));

        assert!(timeout > 0);
    }

    #[test]
    fn floors_a_context_missing_both_timestamps_to_the_minimum_poll_window() {
        assert_eq!(
            poll_timeout_ms(&malformed_window(json!({}))),
            MIN_POLL_SECONDS * 1000
        );
    }

    #[test]
    fn floors_a_context_missing_expires_at_to_the_minimum_poll_window() {
        let window = malformed_window(json!({ "created_at": CREATED_AT }));
        assert_eq!(poll_timeout_ms(&window), MIN_POLL_SECONDS * 1000);
    }

    #[test]
    fn floors_a_context_missing_created_at_to_the_minimum_poll_window() {
        let window = malformed_window(json!({ "expires_at": CREATED_AT }));
        assert_eq!(poll_timeout_ms(&window), MIN_POLL_SECONDS * 1000);
    }

    #[test]
    fn floors_a_context_with_null_timestamps_to_the_minimum_poll_window() {
        let window = malformed_window(json!({ "created_at": null, "expires_at": null }));
        assert_eq!(poll_timeout_ms(&window), MIN_POLL_SECONDS * 1000);
    }

    #[test]
    fn computes_the_real_window_when_timestamps_arrive_as_numeric_strings() {
        let window = malformed_window(json!({
            "created_at": CREATED_AT.to_string(),
            "expires_at": (CREATED_AT + RP_CONTEXT_TTL_SECONDS).to_string(),
        }));

        assert_eq!(
            poll_timeout_ms(&window),
            (RP_CONTEXT_TTL_SECONDS - POLL_SAFETY_MARGIN_SECONDS) * 1000
        );
    }

    #[test]
    fn always_returns_a_finite_window_at_least_the_floor_for_any_malformed_shape() {
        let shapes = [
            json!({}),
            json!({ "created_at": CREATED_AT }),
            json!({ "expires_at": CREATED_AT }),
            json!({ "created_at": null, "expires_at": null }),
            json!({ "created_at": "soon", "expires_at": [1] }),
        ];

        for shape in shapes {
            assert!(poll_timeout_ms(&malformed_window(shape)) >= MIN_POLL_SECONDS * 1000);
        }
    }
}
