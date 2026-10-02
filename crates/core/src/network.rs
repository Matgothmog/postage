use serde::Deserialize;

/// Gas one attestation costs on Arc at 25 gwei, measured. Used to state the
/// sponsorship pool in the unit that means something: people onboarded.
pub const ATTESTATION_COST: u128 = 75_395 * 25_000_000_000;

/// The slice of the subgraph's `Overview` query the pure helpers read. `BigInt`
/// columns arrive as decimal strings; counts arrive as numbers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vault {
    pub total_funded: String,
    pub to_treasury: String,
    pub to_sponsorship: String,
    pub refilled_to_relayer: String,
    pub funding_events: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enclave {
    pub id: String,
    pub measurement: String,
    pub revoked: bool,
    pub registered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanAttestation {
    pub id: String,
    pub attested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxSummary {
    pub id: String,
    pub floor_price: String,
    pub received_count: u64,
    pub earned: String,
    pub claimed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenderSummary {
    pub id: String,
    pub paid_count: u64,
    pub total_paid: String,
    pub spam_reports: u64,
    pub spam_rate: String,
    pub human_until: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IdRef {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payment {
    pub id: String,
    pub tier: String,
    pub amount: String,
    pub to_vault: String,
    pub reported_as_spam: bool,
    pub paid_at: String,
    pub tx: String,
    pub sender: IdRef,
    pub inbox: IdRef,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub vaults: Vec<Vault>,
    pub enclaves: Vec<Enclave>,
    pub human_attestations: Vec<HumanAttestation>,
    pub inboxes: Vec<InboxSummary>,
    pub senders: Vec<SenderSummary>,
    pub payments: Vec<Payment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAggregates<'a> {
    pub sponsored: u128,
    pub earned: u128,
    pub delivered: u64,
    pub signer: Option<&'a Enclave>,
}

/// A subgraph `BigInt` column read back as a number. The subgraph only ever
/// writes digits, so an unreadable value counts as zero rather than taking
/// the whole overview page down.
fn big_int(column: &str) -> u128 {
    column.parse().unwrap_or(0)
}

/// The four numbers the page's header and footer sections need beyond a
/// straight render of what the subgraph returned.
pub fn derive_aggregates(data: &Overview) -> NetworkAggregates<'_> {
    let sponsored = data
        .vaults
        .first()
        .map_or(0, |vault| big_int(&vault.to_sponsorship) / ATTESTATION_COST);
    let earned = data
        .inboxes
        .iter()
        .map(|inbox| big_int(&inbox.earned))
        .sum();
    let delivered = data.inboxes.iter().map(|inbox| inbox.received_count).sum();
    let signer = data.enclaves.iter().find(|enclave| !enclave.revoked);
    NetworkAggregates {
        sponsored,
        earned,
        delivered,
        signer,
    }
}

/// PostageEscrow emits `msg.value - toVault` as Payment.amount, the inbox's
/// net share, not what the sender paid. The full amount the sender's wallet
/// left is `amount + toVault`.
pub fn payment_total(amount: &str, to_vault: &str) -> u128 {
    big_int(amount) + big_int(to_vault)
}

/// A credential that has run out says nothing about who is sending now, so it
/// stops counting the moment it lapses. `now` is epoch seconds.
pub fn still_human(human_until: Option<&str>, now: i64) -> bool {
    human_until
        .and_then(|until| until.parse::<i64>().ok())
        .is_some_and(|until| until > now)
}

/// Whole units only. A feed that says "14 minutes ago" beside "3 hours ago"
/// reads at a glance; one that says "14 minutes 6 seconds" does not.
pub fn since(seconds: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(seconds).max(0);
    match elapsed {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{}m ago", elapsed / 60),
        3600..=86_399 => format!("{}h ago", elapsed / 3600),
        _ => format!("{}d ago", elapsed / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn inbox(received_count: u64, earned: &str) -> InboxSummary {
        InboxSummary {
            id: "0xa".to_owned(),
            floor_price: "0".to_owned(),
            received_count,
            earned: earned.to_owned(),
            claimed: "0".to_owned(),
        }
    }

    fn enclave(id: &str, revoked: bool) -> Enclave {
        Enclave {
            id: id.to_owned(),
            measurement: "0x00".to_owned(),
            revoked,
            registered_at: "0".to_owned(),
        }
    }

    fn vault_sponsoring(to_sponsorship: String) -> Vault {
        Vault {
            total_funded: "0".to_owned(),
            to_treasury: "0".to_owned(),
            to_sponsorship,
            refilled_to_relayer: "0".to_owned(),
            funding_events: 0,
        }
    }

    #[test]
    fn derive_aggregates_sums_what_recipients_kept_not_what_was_charged() {
        let data = Overview {
            inboxes: vec![inbox(1, "800"), inbox(2, "1200")],
            ..Overview::default()
        };

        assert_eq!(derive_aggregates(&data).earned, 2000);
    }

    #[test]
    fn derive_aggregates_counts_every_message_an_inbox_received() {
        let data = Overview {
            inboxes: vec![inbox(3, "0"), inbox(5, "0")],
            ..Overview::default()
        };

        assert_eq!(derive_aggregates(&data).delivered, 8);
    }

    #[test]
    fn derive_aggregates_prices_the_sponsorship_pool_in_whole_attestations_rounded_down() {
        let data = Overview {
            vaults: vec![vault_sponsoring((ATTESTATION_COST * 3 + 1).to_string())],
            ..Overview::default()
        };

        assert_eq!(derive_aggregates(&data).sponsored, 3);
    }

    #[test]
    fn derive_aggregates_prices_sponsorship_as_zero_when_no_vault_has_been_indexed_yet() {
        assert_eq!(derive_aggregates(&Overview::default()).sponsored, 0);
    }

    #[test]
    fn derive_aggregates_picks_the_first_enclave_that_has_not_been_revoked() {
        let data = Overview {
            enclaves: vec![enclave("0xrevoked", true), enclave("0xlive", false)],
            ..Overview::default()
        };

        assert_eq!(
            derive_aggregates(&data).signer.map(|e| e.id.as_str()),
            Some("0xlive")
        );
    }

    #[test]
    fn derive_aggregates_has_no_signer_when_every_registered_key_is_revoked() {
        let data = Overview {
            enclaves: vec![enclave("0xrevoked", true)],
            ..Overview::default()
        };

        assert_eq!(derive_aggregates(&data).signer, None);
    }

    /// PostageEscrow emits `msg.value - toVault` as Payment.amount, so amount
    /// alone understates what the sender's wallet left by the vault's cut.
    #[test]
    fn payment_total_reconstructs_the_full_amount_the_sender_paid_not_the_inboxs_net_share() {
        let total = payment_total("800000000000000000", "200000000000000000");

        assert_eq!(total, 1_000_000_000_000_000_000);
    }

    #[test]
    fn still_human_is_false_once_a_credential_has_lapsed() {
        assert!(!still_human(None, NOW));
        assert!(!still_human(Some(&(NOW - 1).to_string()), NOW));
    }

    #[test]
    fn still_human_is_true_while_a_credential_has_time_left() {
        assert!(still_human(Some(&(NOW + 60).to_string()), NOW));
    }

    #[test]
    fn since_reads_recent_payments_as_just_now() {
        assert_eq!(since(NOW, NOW), "just now");
        assert_eq!(since(NOW - 59, NOW), "just now");
    }

    #[test]
    fn since_rounds_down_to_whole_minutes_then_hours_then_days() {
        assert_eq!(since(NOW - 120, NOW), "2m ago");
        assert_eq!(since(NOW - 2 * 3600, NOW), "2h ago");
        assert_eq!(since(NOW - 2 * 86_400, NOW), "2d ago");
    }

    #[test]
    fn since_clamps_a_timestamp_from_the_future_to_zero_elapsed_rather_than_going_negative() {
        assert_eq!(since(NOW + 3600, NOW), "just now");
    }
}
