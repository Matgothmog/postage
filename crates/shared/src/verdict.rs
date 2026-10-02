use serde::{Deserialize, Serialize};

/// What the worker should do with an inbound message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GatewayAction {
    Forward,
    Hold,
    Reject,
}

/// The one message a held sender is written back, carried inside a `hold`
/// verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayNotice {
    pub subject: String,
    pub html: String,
    pub text: String,
}

/// The gateway's decision about one inbound message, exactly as it crosses the
/// wire between the web gateway and the mail worker. Absent fields are omitted
/// rather than serialized as null, matching the TypeScript `JSON.stringify`
/// output; unknown fields the gateway rides along are ignored on read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayVerdict {
    pub action: GatewayAction,
    /// Verified destination, on `forward` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Key the message is held under, on `hold` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Epoch seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_until: Option<u64>,
    /// Absent (or null on the wire) when answering the sender would mean
    /// mailing someone whose name was forged; the SMTP refusal then carries
    /// the link instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<GatewayNotice>,
    /// What to say inside the SMTP session if we do not write back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce: Option<String>,
}

impl GatewayVerdict {
    fn bare(action: GatewayAction) -> Self {
        Self {
            action,
            to: None,
            reason: None,
            token: None,
            held_until: None,
            notice: None,
            bounce: None,
        }
    }

    pub fn reject(reason: impl Into<String>, bounce: Option<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            bounce,
            ..Self::bare(GatewayAction::Reject)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reject_with_reason_only_omits_every_other_field() {
        let verdict = GatewayVerdict::reject("unknown_inbox", None);
        assert_eq!(
            serde_json::to_value(&verdict).unwrap(),
            json!({"action": "reject", "reason": "unknown_inbox"})
        );
    }

    #[test]
    fn reject_with_bounce_serializes_both_fields() {
        let verdict = GatewayVerdict::reject("dangerous", Some("Not delivered".into()));
        assert_eq!(
            serde_json::to_string(&verdict).unwrap(),
            r#"{"action":"reject","reason":"dangerous","bounce":"Not delivered"}"#
        );
    }

    #[test]
    fn forward_serializes_destination_and_reason() {
        let verdict = GatewayVerdict {
            to: Some("me@example.com".into()),
            reason: Some("allowlisted".into()),
            ..GatewayVerdict::bare(GatewayAction::Forward)
        };
        assert_eq!(
            serde_json::to_string(&verdict).unwrap(),
            r#"{"action":"forward","to":"me@example.com","reason":"allowlisted"}"#
        );
    }

    #[test]
    fn hold_with_notice_matches_the_typescript_wire_format() {
        let wire = r#"{"action":"hold","reason":"commercial","token":"tok","held_until":1760000000,"notice":{"subject":"s","html":"<p>h</p>","text":"t"}}"#;
        let verdict: GatewayVerdict = serde_json::from_str(wire).unwrap();
        assert_eq!(verdict.action, GatewayAction::Hold);
        assert_eq!(verdict.held_until, Some(1_760_000_000));
        assert_eq!(
            verdict.notice.as_ref().map(|n| n.subject.as_str()),
            Some("s")
        );
        assert_eq!(serde_json::to_string(&verdict).unwrap(), wire);
    }

    #[test]
    fn null_notice_reads_as_absent() {
        let verdict: GatewayVerdict =
            serde_json::from_str(r#"{"action":"hold","token":"t","notice":null}"#).unwrap();
        assert_eq!(verdict.notice, None);
    }

    #[test]
    fn extra_fields_the_gateway_rides_along_are_ignored() {
        let wire = r#"{"action":"forward","to":"a@b.c","verdict":{"tier":"human"},"tier":"human"}"#;
        let verdict: GatewayVerdict = serde_json::from_str(wire).unwrap();
        assert_eq!(verdict.to.as_deref(), Some("a@b.c"));
    }

    #[test]
    fn unknown_action_is_rejected() {
        assert!(serde_json::from_str::<GatewayVerdict>(r#"{"action":"explode"}"#).is_err());
    }

    #[test]
    fn missing_action_is_rejected() {
        assert!(serde_json::from_str::<GatewayVerdict>(r#"{"to":"a@b.c"}"#).is_err());
    }
}
