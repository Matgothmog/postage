//! Shared page chrome and the Tailwind class strings every screen reuses.
//! Replaces `web/src/components/chrome.tsx`.
//!
//! Class strings are literals on purpose: Tailwind finds them by scanning
//! these sources, so a name assembled at runtime would generate no CSS.

use leptos::prelude::*;
use leptos_router::components::A;

pub const PRIMARY_BUTTON: &str = "inline-flex items-center justify-center rounded-xl bg-accent px-5 py-3 text-sm font-medium text-on-accent transition hover:bg-accent-hi focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:opacity-40";

pub const SECONDARY_BUTTON: &str = "inline-flex items-center justify-center rounded-xl border border-line-strong bg-surface-2 px-5 py-3 text-sm font-medium text-fg transition hover:border-accent-hi focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg disabled:opacity-40";

pub const QUIET_BUTTON: &str = "inline-flex items-center justify-center rounded-lg px-3 py-2 text-sm text-muted transition hover:text-fg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-hi focus-visible:ring-offset-2 focus-visible:ring-offset-bg";

/// Text inputs; used by the claim form.
pub const FIELD: &str = "w-full rounded-xl border border-line-strong bg-surface px-4 py-3 text-[15px] text-fg outline-none transition placeholder:text-faint focus:border-accent focus:ring-2 focus:ring-accent/30";

#[component]
fn Wordmark() -> impl IntoView {
    view! {
        <A href="/" attr:class="group inline-flex items-center gap-2.5">
            <span class="grid h-6 w-6 place-items-center rounded-[5px] bg-accent text-[11px] font-bold text-on-accent">
                "P"
            </span>
            <span class="text-[13px] font-semibold uppercase tracking-[0.22em] text-fg">
                "Postage"
            </span>
        </A>
    }
}

#[component]
pub fn SiteHeader(#[prop(optional, into)] actions: ViewFn) -> impl IntoView {
    view! {
        <header class="border-b border-line">
            <div class="mx-auto flex w-full max-w-5xl items-center justify-between px-6 py-5">
                <Wordmark />
                <nav class="flex items-center gap-1 text-sm">
                    {actions.run()}
                </nav>
            </div>
        </header>
    }
}

#[component]
pub fn SiteFooter() -> impl IntoView {
    view! {
        <footer class="mt-24 border-t border-line">
            <div class="mx-auto flex w-full max-w-5xl flex-col gap-3 px-6 py-8 text-xs text-faint sm:flex-row sm:items-center sm:justify-between">
                <p>"Postage"</p>
                <p class="font-mono">"USDC on Arc"</p>
            </div>
        </footer>
    }
}

/// Header, the flex-grown content slot, footer. `actions` is the header nav
/// slot each page fills differently (a sign-in control, a link).
#[component]
pub fn Shell(#[prop(optional, into)] actions: ViewFn, children: Children) -> impl IntoView {
    view! {
        <div class="flex min-h-dvh flex-col">
            <SiteHeader actions=actions />
            <div class="flex-1">{children()}</div>
            <SiteFooter />
        </div>
    }
}

/// The product drawn as the thing it is named after: a flat surface with the
/// `.glow` wash.
#[component]
pub fn AddressCard(
    #[prop(into)] handle: String,
    #[prop(into)] price: String,
    #[prop(into)] caption: String,
) -> impl IntoView {
    view! {
        <div class="glow w-full max-w-xs rounded-2xl border border-line-strong bg-surface px-7 py-8">
            <div class="flex items-start justify-between">
                <span class="text-[10px] font-semibold uppercase tracking-[0.2em] text-accent">
                    "Postage"
                </span>
                <span class="rounded-full border border-accent px-2 py-0.5 font-mono text-[10px] text-accent-hi">
                    {price}
                </span>
            </div>
            <p class="mt-7 font-mono text-[15px] leading-snug break-all text-fg">{handle}</p>
            <p class="mt-1.5 text-xs text-muted">{caption}</p>
        </div>
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalloutTone {
    Good,
    Bad,
    Quiet,
}

impl CalloutTone {
    fn classes(self) -> &'static str {
        match self {
            Self::Good => "border-good/30 bg-good-soft text-good",
            Self::Bad => "border-bad/30 bg-bad-soft text-bad",
            Self::Quiet => "border-line bg-surface text-muted",
        }
    }
}

#[component]
pub fn Callout(
    tone: CalloutTone,
    #[prop(into)] title: String,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class=format!("rounded-xl border p-4 text-sm {}", tone.classes())>
            <p class="font-medium">{title}</p>
            {children.map(|children| view! { <div class="mt-1 opacity-90">{children()}</div> })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tone_has_its_own_palette() {
        let tones = [CalloutTone::Good, CalloutTone::Bad, CalloutTone::Quiet];
        let classes: Vec<_> = tones.iter().map(|tone| tone.classes()).collect();
        assert!(classes.iter().all(|c| !c.is_empty()));
        assert_eq!(
            classes.len(),
            classes
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        );
    }
}
