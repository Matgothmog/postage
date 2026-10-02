//! The two messages Postage writes itself (`web/src/lib/mail.ts`), as text.
//!
//! Neither is a forward: a released message never passes through here,
//! because it has to leave as the bytes that arrived. Sending them is the
//! server's job; what they say is decided here, where it can be tested
//! without a mail provider.

use crate::handle::postage_address;
use crate::verification::CODE_TTL_SECONDS;

/// What a claimant sees in their inbox's subject line.
pub fn verification_code_subject(code: &str) -> String {
    format!("{code} is your Postage code")
}

/// The code that proves whoever is claiming a handle can read the address
/// they are pointing it at, and what to do if it was not them.
pub fn verification_code_text(handle: &str, code: &str) -> String {
    [
        code.to_owned(),
        String::new(),
        format!(
            "Someone pointed {} at this address. Enter the code",
            postage_address(handle)
        ),
        "above to confirm it was you.".to_owned(),
        String::new(),
        "Cloudflare, who carries the mail, is sending a separate confirmation too.".to_owned(),
        "Both are needed before anything forwards here.".to_owned(),
        String::new(),
        "Not you? Ignore both. Nothing forwards without confirming, and this code".to_owned(),
        format!("expires in {} minutes.", CODE_TTL_SECONDS / 60),
    ]
    .join("\n")
}

/// A message a cleared sender pasted back in, with the line that says who
/// cleared the gate. It goes out under our own name with theirs in Reply-To,
/// so this footer is how the recipient learns who wrote it.
pub fn relayed_text(body: &str, from: &str, handle: &str) -> String {
    [
        body.to_owned(),
        String::new(),
        "\u{2014}".to_owned(),
        format!(
            "{from} cleared the gate. Sent to {}.",
            postage_address(handle)
        ),
        "Reply goes straight to them.".to_owned(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_leads_the_subject() {
        assert_eq!(
            verification_code_subject("123456"),
            "123456 is your Postage code"
        );
    }

    /// The whole text, written out once: what a stranger reads when someone
    /// aims a handle at their inbox.
    #[test]
    fn the_verification_mail_says_what_happened_and_how_long_the_code_lasts() {
        assert_eq!(
            verification_code_text("demo", "123456"),
            "123456\n\
             \n\
             Someone pointed demo@usepostage.com at this address. Enter the code\n\
             above to confirm it was you.\n\
             \n\
             Cloudflare, who carries the mail, is sending a separate confirmation too.\n\
             Both are needed before anything forwards here.\n\
             \n\
             Not you? Ignore both. Nothing forwards without confirming, and this code\n\
             expires in 15 minutes."
        );
    }

    /// The TypeScript divided in floating point; a lifetime that is not whole
    /// minutes would have printed a fraction there and a truncation here.
    #[test]
    fn the_code_lifetime_is_whole_minutes() {
        assert_eq!(CODE_TTL_SECONDS % 60, 0);
    }

    #[test]
    fn a_relayed_message_keeps_its_body_and_names_who_cleared_the_gate() {
        assert_eq!(
            relayed_text("Hello there.", "alice@example.com", "demo"),
            "Hello there.\n\
             \n\
             \u{2014}\n\
             alice@example.com cleared the gate. Sent to demo@usepostage.com.\n\
             Reply goes straight to them."
        );
    }
}
