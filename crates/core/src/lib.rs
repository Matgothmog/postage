//! Pure domain logic: no network, database or clock access lives here.

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
