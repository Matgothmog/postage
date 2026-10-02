use serde::{Deserialize, Serialize};

/// Path of the worker's only route; everything else answers 404.
pub const RELEASE_PATH: &str = "/release";

/// Header carrying the shared secret; a mismatch answers 401.
pub const RELEASE_SECRET_HEADER: &str = "x-postage-secret";

/// Body of the web gateway's `POST /release` to the mail worker: the held
/// message's token and the verified destination to send it to. Both are
/// required; the worker answers 400 when either is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRequest {
    pub token: String,
    pub to: String,
}

/// The worker's 200 body once the message has gone out. Failures are plain
/// text with a status (400, 401, 404, 502, 503), not this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseResponse {
    pub sent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_request_serializes_token_then_to() {
        let request = ReleaseRequest {
            token: "tok".into(),
            to: "me@example.com".into(),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"token":"tok","to":"me@example.com"}"#
        );
    }

    #[test]
    fn release_request_without_destination_is_rejected() {
        assert!(serde_json::from_str::<ReleaseRequest>(r#"{"token":"tok"}"#).is_err());
    }

    #[test]
    fn release_response_matches_the_worker_body() {
        let response: ReleaseResponse = serde_json::from_str(r#"{"sent":true}"#).unwrap();
        assert!(response.sent);
        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"sent":true}"#
        );
    }

    #[test]
    fn secret_header_name_is_lowercase() {
        assert_eq!(RELEASE_SECRET_HEADER, "x-postage-secret");
        assert_eq!(RELEASE_PATH, "/release");
    }
}
