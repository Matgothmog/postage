//! `MESSAGE_ID_SECRET` and the purpose-bound keys derived from it.
//!
//! One piece of key material derives several independent keys, which is
//! exactly what a domain-separating HKDF label is for: `postage:wallet-nonce`
//! and `postage:verification-code` cannot collide, and neither can reproduce
//! the raw secret that message ids are HMACed with. Each derived key is its own
//! type, so a key made for one purpose cannot be handed to another's function.

use std::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub(crate) type HmacSha256 = Hmac<Sha256>;

const DERIVED_KEY_BYTES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    #[error("MESSAGE_ID_SECRET is empty")]
    Empty,
    /// Unreachable for the fixed 32-byte length used here, but HKDF and HMAC
    /// both report length errors through a `Result`, and failing closed beats
    /// panicking on a path every sign-in runs through.
    #[error("could not derive the {0} key")]
    Derivation(&'static str),
}

/// The operator's `MESSAGE_ID_SECRET`, as raw UTF-8 bytes. `Debug` never shows
/// it, so a stray `{:?}` in a log line cannot leak it.
#[derive(Clone)]
pub struct MessageIdSecret(Vec<u8>);

impl MessageIdSecret {
    /// Refuses an empty value, which would make every derived key a public
    /// constant.
    pub fn new(value: impl Into<String>) -> Result<Self, SecretError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SecretError::Empty);
        }
        Ok(Self(value.into_bytes()))
    }
}

impl fmt::Debug for MessageIdSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MessageIdSecret(<redacted>)")
    }
}

/// HKDF-SHA256 with an empty salt, as `hkdfSync("sha256", root, "", info, 32)`
/// does, keyed straight into an HMAC so callers never handle the bytes.
fn derive_mac(secret: &MessageIdSecret, info: &'static str) -> Result<HmacSha256, SecretError> {
    let mut okm = [0u8; DERIVED_KEY_BYTES];
    Hkdf::<Sha256>::new(Some(&[]), &secret.0)
        .expand(info.as_bytes(), &mut okm)
        .map_err(|_| SecretError::Derivation(info))?;
    HmacSha256::new_from_slice(&okm).map_err(|_| SecretError::Derivation(info))
}

macro_rules! derived_key {
    ($(#[$doc:meta])* $name:ident, $info:literal) => {
        $(#[$doc])*
        #[derive(Clone)]
        pub struct $name(HmacSha256);

        impl $name {
            pub fn derive(secret: &MessageIdSecret) -> Result<Self, SecretError> {
                derive_mac(secret, $info).map(Self)
            }

            /// A fresh MAC under this key, ready for one message.
            pub(crate) fn mac(&self) -> HmacSha256 {
                self.0.clone()
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }
    };
}

derived_key!(
    /// Signs wallet nonces (`wallet_nonce`).
    WalletNonceKey,
    "postage:wallet-nonce"
);

derived_key!(
    /// Hashes emailed verification codes (`verification`).
    VerificationKey,
    "postage:verification-code"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_secret_is_refused() {
        assert_eq!(MessageIdSecret::new("").unwrap_err(), SecretError::Empty);
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        let secret = MessageIdSecret::new("hunter2-super-secret").unwrap();
        let nonce_key = WalletNonceKey::derive(&secret).unwrap();
        let code_key = VerificationKey::derive(&secret).unwrap();

        let shown = format!("{secret:?} {nonce_key:?} {code_key:?}");

        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("<redacted>"));
    }
}
