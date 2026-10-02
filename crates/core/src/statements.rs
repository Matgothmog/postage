//! What a wallet signs to prove a request is really from it. Both ends build
//! the same string, so this pulls in nothing but the domain constant.

use crate::handle::{MAIL_DOMAIN, postage_address};
use crate::js_number::to_js_string;

pub fn claim_statement(handle: &str, destination: &str, wallet: &str, issued_at: f64) -> String {
    [
        "Postage: claim an address".to_owned(),
        format!("Handle: {}", postage_address(handle)),
        format!("Forward to: {}", destination.to_lowercase()),
        format!("Wallet: {}", wallet.to_lowercase()),
        issued_line(issued_at),
    ]
    .join("\n")
}

/// Names the deployment as well as the wallet. The other two carry it already,
/// inside the `postage_address(handle)` they each state; this one has no
/// handle to carry it, so without the domain line a signature collected on a
/// staging copy, or by any other site that asked the same wallet for one,
/// would open this inbox too for as long as the timestamp stayed fresh.
pub fn read_statement(wallet: &str, issued_at: f64) -> String {
    [
        "Postage: read my inbox".to_owned(),
        format!("Domain: {MAIL_DOMAIN}"),
        format!("Wallet: {}", wallet.to_lowercase()),
        issued_line(issued_at),
    ]
    .join("\n")
}

/// Names the handle as well as the wallet, so a signature collected for one
/// claim cannot confirm another.
pub fn confirm_statement(handle: &str, wallet: &str, issued_at: f64) -> String {
    [
        "Postage: confirm my code".to_owned(),
        format!("Handle: {}", postage_address(handle)),
        format!("Wallet: {}", wallet.to_lowercase()),
        issued_line(issued_at),
    ]
    .join("\n")
}

/// The timestamp as JavaScript's template literal wrote it. The reader signs
/// over whatever `Number()` made of a caller's header, fractional or not, so
/// a statement must render that number the way the TypeScript verifier did or
/// a signature it accepted would stop verifying here.
fn issued_line(issued_at: f64) -> String {
    format!("Issued: {}", to_js_string(issued_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALLET: &str = "0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A";
    const ISSUED_AT: f64 = 1_757_332_800.0;

    /// The wire format, written out by hand the one time it is written out
    /// twice: a change here would silently change what an open browser tab
    /// signs.
    #[test]
    fn read_statement_names_the_deployment_alongside_the_wallet_and_the_timestamp() {
        assert_eq!(
            read_statement(WALLET, ISSUED_AT),
            [
                "Postage: read my inbox".to_owned(),
                format!("Domain: {MAIL_DOMAIN}"),
                format!("Wallet: {}", WALLET.to_lowercase()),
                "Issued: 1757332800".to_owned(),
            ]
            .join("\n")
        );
    }

    #[test]
    fn every_statement_a_wallet_is_asked_to_sign_names_the_deployment_asking() {
        assert!(
            read_statement(WALLET, ISSUED_AT).contains(MAIL_DOMAIN),
            "read_statement is unbound"
        );
        assert!(
            claim_statement("demo", "reader@example.com", WALLET, ISSUED_AT).contains(MAIL_DOMAIN),
            "claim_statement is unbound"
        );
        assert!(
            confirm_statement("demo", WALLET, ISSUED_AT).contains(MAIL_DOMAIN),
            "confirm_statement is unbound"
        );
    }

    /// Whatever `Number()` made of the header is what the TypeScript reader
    /// signed over, and it wrote the number back with a template literal.
    #[test]
    fn the_timestamp_is_written_as_javascript_writes_the_number() {
        let issued = |issued_at: f64| {
            read_statement(WALLET, issued_at)
                .lines()
                .last()
                .unwrap()
                .to_owned()
        };
        assert_eq!(issued(1_757_332_800.5), "Issued: 1757332800.5");
        assert_eq!(issued(1e21), "Issued: 1e+21");
        assert_eq!(issued(f64::NAN), "Issued: NaN");
        assert_eq!(issued(f64::NEG_INFINITY), "Issued: -Infinity");
        assert_eq!(issued(-0.0), "Issued: 0");
    }
}
