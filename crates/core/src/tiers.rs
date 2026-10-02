pub use postage_shared::Tier;

/// A tier read back off a stored quote, where it is only a string. Anything
/// unrecognised is treated as commercial, which is the tier an ordinary
/// stranger pays.
pub fn tier_index_of(tier: &str) -> u8 {
    Tier::from_name(tier).unwrap_or(Tier::Commercial).index()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tiers_map_to_their_escrow_index() {
        assert_eq!(tier_index_of("human"), 0);
        assert_eq!(tier_index_of("important"), 1);
        assert_eq!(tier_index_of("commercial"), 2);
        assert_eq!(tier_index_of("dangerous"), 3);
    }

    #[test]
    fn unrecognised_tier_is_priced_as_commercial() {
        assert_eq!(tier_index_of("spam"), 2);
        assert_eq!(tier_index_of(""), 2);
    }
}
