use serde::{Deserialize, Serialize};

/// The four verdicts, in the order the escrow's `Tier` enum puts them in.
///
/// Lives in the shared crate because it crosses the wire as a lowercase
/// string and its position is priced onchain: a reordered enum would misprice
/// every message rather than fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Human,
    Important,
    Commercial,
    Dangerous,
}

impl Tier {
    /// Every tier, in escrow order.
    pub const ALL: [Tier; 4] = [
        Tier::Human,
        Tier::Important,
        Tier::Commercial,
        Tier::Dangerous,
    ];

    /// The tier's position in the escrow's `Tier` enum.
    pub const fn index(self) -> u8 {
        match self {
            Tier::Human => 0,
            Tier::Important => 1,
            Tier::Commercial => 2,
            Tier::Dangerous => 3,
        }
    }

    /// The wire spelling, which is also what stored quotes hold.
    pub const fn as_str(self) -> &'static str {
        match self {
            Tier::Human => "human",
            Tier::Important => "important",
            Tier::Commercial => "commercial",
            Tier::Dangerous => "dangerous",
        }
    }

    /// Parses the wire spelling; `None` for anything unrecognised.
    pub fn from_name(name: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|tier| tier.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_listed_in_escrow_order() {
        let indexes: Vec<u8> = Tier::ALL.iter().map(|tier| tier.index()).collect();
        assert_eq!(indexes, vec![0, 1, 2, 3]);
    }

    #[test]
    fn tier_serializes_as_its_lowercase_name() {
        assert_eq!(
            serde_json::to_string(&Tier::Dangerous).unwrap(),
            "\"dangerous\""
        );
    }

    #[test]
    fn tier_deserializes_from_its_lowercase_name() {
        let tier: Tier = serde_json::from_str("\"important\"").unwrap();
        assert_eq!(tier, Tier::Important);
    }

    #[test]
    fn wire_name_round_trips_through_from_name() {
        for tier in Tier::ALL {
            assert_eq!(Tier::from_name(tier.as_str()), Some(tier));
        }
    }

    #[test]
    fn unknown_tier_name_is_rejected() {
        assert_eq!(Tier::from_name("Human"), None);
        assert!(serde_json::from_str::<Tier>("\"spam\"").is_err());
    }
}
