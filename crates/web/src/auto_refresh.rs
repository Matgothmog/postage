//! "re-reading in 12s": the ledger is a view of a chain that keeps moving, so
//! the page re-reads itself rather than showing whatever was true when it was
//! opened. Replaces `web/src/app/network/AutoRefresh.tsx`.
//!
//! The countdown is read off a deadline rather than decremented, so what it
//! says and what it does cannot drift apart however late the timer fires.
//! Whether a tick turns into a request is the owner's decision (it skips one
//! while a read is still out); this only keeps time.

use std::time::Duration;

use leptos::leptos_dom::helpers::set_interval_with_handle;
use leptos::prelude::*;

/// How often the label and the deadline are checked (the TS ticked at 250 ms).
const TICK: Duration = Duration::from_millis(250);

/// Whole seconds still to wait, rounded up, never negative.
fn seconds_left(remaining_millis: f64) -> u64 {
    (remaining_millis.max(0.0) / 1000.0).ceil() as u64
}

#[component]
pub fn AutoRefresh(every: Duration, on_refresh: Callback<()>) -> impl IntoView {
    let period_millis = every.as_secs_f64() * 1000.0;
    let left = RwSignal::new(seconds_left(period_millis));
    let deadline = StoredValue::new(js_sys::Date::now() + period_millis);

    let tick = move || {
        let now = js_sys::Date::now();
        let remaining = deadline.get_value() - now;
        if remaining > 0.0 {
            left.set(seconds_left(remaining));
            return;
        }
        // Re-armed from now, not from the missed deadline, so a tab that was
        // asleep for an hour asks once on waking rather than catching up.
        deadline.set_value(now + period_millis);
        left.set(seconds_left(period_millis));
        on_refresh.run(());
    };

    match set_interval_with_handle(tick, TICK) {
        Ok(interval) => on_cleanup(move || interval.clear()),
        Err(cause) => leptos::logging::error!("could not start the ledger refresh: {cause:?}"),
    }

    view! {
        <span class="inline-flex items-center gap-2 text-xs text-faint">
            <span class="relative flex h-1.5 w-1.5">
                <span class="absolute inline-flex h-full w-full animate-ping rounded-full bg-good opacity-70" />
                <span class="relative inline-flex h-1.5 w-1.5 rounded-full bg-good" />
            </span>
            "re-reading in " {move || left.get()} "s"
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_countdown_rounds_up_so_it_never_says_zero_before_it_fires() {
        assert_eq!(seconds_left(20_000.0), 20);
        assert_eq!(seconds_left(19_001.0), 20);
        assert_eq!(seconds_left(1.0), 1);
    }

    #[test]
    fn a_passed_deadline_reads_zero() {
        assert_eq!(seconds_left(0.0), 0);
        assert_eq!(seconds_left(-5_000.0), 0);
    }
}
