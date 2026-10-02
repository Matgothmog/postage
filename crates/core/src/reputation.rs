//! What a sender's history says about them, as the pricing reads it
//! (`web/src/lib/reputation.ts`). The queries and the fold over their answers
//! live here; fetching them is the server's job.

use serde::Deserialize;

use crate::pricing::SenderSignals;
use crate::wallet_proof::js_number;

/// The ENS subgraph on the decentralized network, queried through the
/// gateway. It gives a sender's history outside Postage.
pub const ENS_SUBGRAPH: &str = "5XqPmWe6gjyrJtFn9cLy237i4cWw2j9HcUJEXsP5qGtH";

/// Our own subgraph: what this sender has done inside Postage.
pub const POSTAGE_HISTORY: &str = r#"
  query SenderHistory($wallet: ID!) {
    sender(id: $wallet) {
      paidCount
      spamReports
      spamRate
    }
  }
"#;

pub const ENS_OWNED: &str = r#"
  query NamesOwned($wallet: String!) {
    domains(first: 5, where: { owner: $wallet }) {
      createdAt
    }
  }
"#;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SenderHistory {
    pub sender: Option<PostageSender>,
}

/// `spamRate` is a subgraph `BigDecimal`, so it arrives as a string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostageSender {
    pub paid_count: u64,
    pub spam_reports: u64,
    pub spam_rate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EnsNames {
    pub domains: Vec<EnsDomain>,
}

/// `createdAt` is a subgraph `BigInt`: epoch seconds as a decimal string.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsDomain {
    pub created_at: String,
}

/// Folds whatever the two lookups returned into the signals `quote` reads.
/// A lookup that failed is passed as `None` (or no domains) and counts as a
/// sender with no history, which softens the price rather than blocking the
/// message.
pub fn signals_from(sender: Option<&PostageSender>, domains: &[EnsDomain]) -> SenderSignals {
    SenderSignals {
        paid_count: sender.map_or(0, |sender| sender.paid_count),
        spam_reports: sender.map_or(0, |sender| sender.spam_reports),
        // `Number(text)`, NaN included: pricing compares it, and a NaN rate
        // fails both comparisons exactly as it did in TypeScript.
        spam_rate: sender.map_or(0.0, |sender| js_number(&sender.spam_rate)),
        ens_names: domains.len() as u64,
        oldest_ens_at: oldest(domains),
    }
}

/// `Math.min` over every `createdAt`. One unreadable value makes the whole
/// minimum NaN in JavaScript, which then earns no age credit; `None` earns
/// none here either.
fn oldest(domains: &[EnsDomain]) -> Option<i64> {
    let mut oldest: Option<f64> = None;
    for domain in domains {
        let created = js_number(&domain.created_at);
        if !created.is_finite() {
            return None;
        }
        oldest = Some(oldest.map_or(created, |current| current.min(created)));
    }
    // Saturating float-to-int; a subgraph `BigInt` of seconds is far inside it.
    oldest.map(|seconds| seconds as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(paid: u64, reports: u64, rate: &str) -> PostageSender {
        PostageSender {
            paid_count: paid,
            spam_reports: reports,
            spam_rate: rate.to_owned(),
        }
    }

    fn domains(created: &[&str]) -> Vec<EnsDomain> {
        created
            .iter()
            .map(|at| EnsDomain {
                created_at: (*at).to_owned(),
            })
            .collect()
    }

    #[test]
    fn no_history_and_no_names_is_a_blank_sender() {
        assert_eq!(
            signals_from(None, &[]),
            SenderSignals {
                paid_count: 0,
                spam_reports: 0,
                spam_rate: 0.0,
                ens_names: 0,
                oldest_ens_at: None,
            }
        );
    }

    #[test]
    fn a_known_sender_carries_its_counts_and_parsed_rate() {
        let signals = signals_from(Some(&sender(4, 1, "0.25")), &[]);
        assert_eq!(signals.paid_count, 4);
        assert_eq!(signals.spam_reports, 1);
        assert_eq!(signals.spam_rate, 0.25);
    }

    #[test]
    fn an_unreadable_spam_rate_is_nan_as_javascript_number_makes_it() {
        assert!(
            signals_from(Some(&sender(1, 0, "abc")), &[])
                .spam_rate
                .is_nan()
        );
    }

    #[test]
    fn the_oldest_name_is_the_smallest_created_at() {
        let signals = signals_from(None, &domains(&["1700000000", "1600000000", "1650000000"]));
        assert_eq!(signals.ens_names, 3);
        assert_eq!(signals.oldest_ens_at, Some(1_600_000_000));
    }

    #[test]
    fn a_name_created_at_zero_is_a_real_timestamp() {
        assert_eq!(signals_from(None, &domains(&["0"])).oldest_ens_at, Some(0));
    }

    #[test]
    fn one_unreadable_created_at_withholds_the_age_but_still_counts_names() {
        let signals = signals_from(None, &domains(&["1600000000", "soon"]));
        assert_eq!(signals.ens_names, 2);
        assert_eq!(signals.oldest_ens_at, None);
    }
}
