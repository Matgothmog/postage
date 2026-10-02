/// Seconds, which is what every timestamp column holds. Callers read the clock
/// (core has no clock access) and convert here, so a millisecond value can
/// never be stored into a column the next query reads as seconds.
pub const fn seconds_from_millis(millis: i64) -> i64 {
    millis.div_euclid(1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds_are_floored_to_seconds() {
        assert_eq!(seconds_from_millis(1_760_000_000_999), 1_760_000_000);
    }

    #[test]
    fn exact_second_boundary_is_unchanged() {
        assert_eq!(seconds_from_millis(5_000), 5);
    }

    #[test]
    fn negative_milliseconds_floor_toward_negative_infinity() {
        assert_eq!(seconds_from_millis(-1), -1);
    }
}
