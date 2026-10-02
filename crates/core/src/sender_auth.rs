//! Whether the receiving server confirmed the envelope sender is who it says
//! (replaces `senderIsAuthenticated` in
//! `web/src/app/api/mail/inbound/route.ts`).
//!
//! Everything the gateway keys on a sender - passes, the per-sender
//! classification budget, the hold notice written back - is keyed on the
//! envelope sender (SMTP `MAIL FROM`). So the only evidence that counts is
//! evidence about that address:
//!
//! - SPF checks the envelope domain itself. A pass counts only alongside a
//!   verified DKIM signature, as it always has: the worker collapses
//!   disagreeing Authentication-Results to null, and null must not read as
//!   clean.
//! - DMARC checks the visible `From:` header, not the envelope. A DMARC pass
//!   says nothing about the envelope unless the two name the same domain;
//!   otherwise anyone could sign for their own domain and write someone
//!   else's address on the envelope. Alignment here is strict: the domains
//!   must be equal, not merely share an organisational domain.

/// What the receiving server concluded about one message, and the two
/// addresses those conclusions are about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SenderEvidence<'a> {
    /// The SMTP envelope sender, which everything is keyed on.
    pub envelope_from: &'a str,
    /// The `From:` header address DMARC evaluated. `None` when the worker did
    /// not send one, which leaves DMARC proving nothing about the envelope.
    pub header_from: Option<&'a str>,
    pub spf: Option<&'a str>,
    pub dkim: Option<&'a str>,
    pub dmarc: Option<&'a str>,
}

/// True when the evidence proves the envelope sender's domain: SPF and DKIM
/// both verified, or DMARC passed for a `From:` domain equal to the
/// envelope's.
pub fn sender_is_authenticated(evidence: &SenderEvidence<'_>) -> bool {
    let spf_and_signature = passed(evidence.spf) && passed(evidence.dkim);
    let aligned_dmarc = passed(evidence.dmarc) && dmarc_covers_envelope(evidence);
    spf_and_signature || aligned_dmarc
}

/// Only the exact word: `softfail`, `neutral`, a missing result and anything
/// else all read as not passed.
fn passed(result: Option<&str>) -> bool {
    result == Some("pass")
}

fn dmarc_covers_envelope(evidence: &SenderEvidence<'_>) -> bool {
    let Some(header_from) = evidence.header_from else {
        return false;
    };
    match (domain_of(evidence.envelope_from), domain_of(header_from)) {
        (Some(envelope), Some(header)) => envelope == header,
        _ => false,
    }
}

/// The domain of an address, lower-cased with one trailing dot removed, or
/// `None` when there is no non-empty domain to compare (the null sender
/// `<>` among them).
pub fn domain_of(address: &str) -> Option<String> {
    let (_, domain) = address.trim().rsplit_once('@')?;
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    if domain.is_empty() {
        return None;
    }
    Some(domain.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "alice@gmail.com";

    fn evidence<'a>(
        header_from: Option<&'a str>,
        spf: &'a str,
        dkim: &'a str,
        dmarc: &'a str,
    ) -> SenderEvidence<'a> {
        SenderEvidence {
            envelope_from: ALICE,
            header_from,
            spf: Some(spf),
            dkim: Some(dkim),
            dmarc: Some(dmarc),
        }
    }

    #[test]
    fn spf_and_dkim_passing_authenticate_without_dmarc() {
        assert!(sender_is_authenticated(&evidence(
            None, "pass", "pass", "none"
        )));
    }

    /// Stricter than SPF alone, as the TypeScript was: a dkim the worker
    /// collapsed to null must not read as a signature.
    #[test]
    fn spf_pass_without_a_verified_signature_does_not_authenticate() {
        assert!(!sender_is_authenticated(&evidence(
            None, "pass", "none", "none"
        )));
        let unsigned = SenderEvidence {
            dkim: None,
            ..evidence(None, "pass", "none", "none")
        };
        assert!(!sender_is_authenticated(&unsigned));
    }

    #[test]
    fn dmarc_pass_with_the_from_domain_equal_to_the_envelopes_authenticates() {
        let aligned = evidence(Some("alice@gmail.com"), "none", "none", "pass");
        assert!(sender_is_authenticated(&aligned));
    }

    /// The attack: sign for your own domain, write someone else's address on
    /// the envelope.
    #[test]
    fn dmarc_pass_for_a_different_from_domain_does_not_vouch_for_the_envelope() {
        let forged = evidence(Some("x@attacker.example"), "softfail", "pass", "pass");
        assert!(!sender_is_authenticated(&forged));
    }

    #[test]
    fn dmarc_pass_with_no_from_header_reported_does_not_authenticate() {
        assert!(!sender_is_authenticated(&evidence(
            None, "none", "pass", "pass"
        )));
    }

    #[test]
    fn alignment_is_strict_so_a_subdomain_does_not_match_its_parent() {
        let subdomain = evidence(Some("news@mail.gmail.com"), "none", "none", "pass");
        assert!(!sender_is_authenticated(&subdomain));
        let parent = SenderEvidence {
            envelope_from: "bounce@mail.gmail.com",
            ..evidence(Some("alice@gmail.com"), "none", "none", "pass")
        };
        assert!(!sender_is_authenticated(&parent));
    }

    #[test]
    fn domains_compare_without_case_or_a_trailing_dot() {
        let shouted = SenderEvidence {
            envelope_from: "Alice@GMAIL.com.",
            ..evidence(Some("<alice@Gmail.Com>"), "none", "none", "pass")
        };
        // The header address arrives bare from the worker; a stray bracket
        // must not align by accident.
        assert!(!sender_is_authenticated(&shouted));
        let bare = SenderEvidence {
            header_from: Some("alice@Gmail.Com"),
            ..shouted
        };
        assert!(sender_is_authenticated(&bare));
    }

    #[test]
    fn softfail_and_neutral_spf_are_not_a_pass() {
        for spf in ["softfail", "neutral", "Pass", "fail", "none"] {
            assert!(
                !sender_is_authenticated(&evidence(None, spf, "pass", "fail")),
                "spf={spf} must not authenticate"
            );
        }
    }

    #[test]
    fn a_dmarc_result_other_than_pass_proves_nothing_even_when_aligned() {
        for dmarc in ["fail", "none", "bestguesspass", "PASS"] {
            let aligned = evidence(Some(ALICE), "none", "none", dmarc);
            assert!(!sender_is_authenticated(&aligned), "dmarc={dmarc}");
        }
    }

    #[test]
    fn a_null_envelope_sender_never_aligns() {
        let bounce = SenderEvidence {
            envelope_from: "",
            ..evidence(Some("mailer-daemon@"), "none", "none", "pass")
        };
        assert!(!sender_is_authenticated(&bounce));
    }

    #[test]
    fn no_evidence_at_all_does_not_authenticate() {
        assert!(!sender_is_authenticated(&SenderEvidence::default()));
    }

    #[test]
    fn domain_of_normalises_and_refuses_what_has_no_domain() {
        assert_eq!(
            domain_of(" Bob@Example.ORG. "),
            Some("example.org".to_owned())
        );
        assert_eq!(
            domain_of("odd\"@\"quote@example.org"),
            Some("example.org".to_owned())
        );
        assert_eq!(domain_of("nobody"), None);
        assert_eq!(domain_of("nobody@"), None);
        assert_eq!(domain_of("nobody@."), None);
        assert_eq!(domain_of(""), None);
    }
}
