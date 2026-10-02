//! Operator-facing error lines, the `console.error(message, { context })` of
//! the TypeScript: an event name, then `key=value` pairs.
//!
//! Lines go to stderr, which is what the host's function logs collect. Under
//! test they are also kept, so a test can say a line was written and that
//! nothing secret was in it.

use std::fmt::Display;

/// Writes one error line: `event key=value ...`.
pub(crate) fn error(event: &str, fields: &[(&str, &dyn Display)]) {
    let line = format_line(event, fields);
    eprintln!("{line}");
    #[cfg(test)]
    captured::record(line);
}

fn format_line(event: &str, fields: &[(&str, &dyn Display)]) -> String {
    let mut line = event.to_owned();
    for (key, value) in fields {
        line.push_str(&format!(" {key}={value}"));
    }
    line
}

#[cfg(test)]
pub(crate) mod captured {
    //! The lines one future writes, kept apart from every other test's.
    //! Task-local rather than global, because tests run in parallel under
    //! `cargo test` and a count over a shared list would include theirs.

    use std::cell::RefCell;
    use std::future::Future;

    tokio::task_local! {
        static LINES: RefCell<Vec<String>>;
    }

    pub(crate) fn record(line: String) {
        // Outside `during` nobody is listening, which is not an error.
        let _ = LINES.try_with(|lines| lines.borrow_mut().push(line));
    }

    /// Runs `future` and returns what it produced with every line it wrote.
    pub(crate) async fn during<F: Future>(future: F) -> (F::Output, Vec<String>) {
        LINES
            .scope(RefCell::new(Vec::new()), async {
                let output = future.await;
                let lines = LINES.with(|lines| lines.take());
                (output, lines)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_the_event_then_each_field_as_key_equals_value() {
        assert_eq!(
            format_line(
                "held message not released",
                &[("handle", &"demo"), ("reason", &"no_inbox")]
            ),
            "held message not released handle=demo reason=no_inbox"
        );
    }

    #[tokio::test]
    async fn a_written_line_is_kept_for_the_future_that_wrote_it() {
        let ((), lines) = captured::during(async {
            error("log module self-test", &[("marker", &"7f3a9c")]);
        })
        .await;
        assert_eq!(lines, ["log module self-test marker=7f3a9c"]);
    }
}
