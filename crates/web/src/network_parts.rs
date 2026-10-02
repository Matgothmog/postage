//! The small presentational pieces of the ledger page. Replaces
//! `web/src/app/network/components.tsx`.

use leptos::prelude::*;

/// One figure of the four across the top.
#[component]
pub fn Stat(
    #[prop(into)] label: String,
    #[prop(into)] value: String,
    #[prop(into)] note: String,
) -> impl IntoView {
    view! {
        <div class="bg-surface p-5">
            <p class="text-[11px] uppercase tracking-[0.14em] text-faint">{label}</p>
            <p class="mt-2 font-mono text-2xl tabular-nums text-fg">{value}</p>
            <p class="mt-1 text-xs text-faint">{note}</p>
        </div>
    }
}

/// One figure of the vault's three.
#[component]
pub fn Flow(
    #[prop(into)] label: String,
    #[prop(into)] value: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="bg-surface p-5">
            <p class="text-[11px] uppercase tracking-[0.14em] text-faint">{label}</p>
            <p class="mt-2 font-mono text-xl tabular-nums text-fg">{value}</p>
            <p class="mt-1 text-xs text-faint">{children()}</p>
        </div>
    }
}

#[component]
pub fn Section(
    title: &'static str,
    #[prop(optional)] hint: Option<&'static str>,
    children: Children,
) -> impl IntoView {
    view! {
        <section class="mt-12">
            <h2 class="text-[15px] font-medium text-fg">{title}</h2>
            {hint
                .map(|hint| {
                    view! { <p class="mt-1 max-w-2xl text-sm leading-relaxed text-muted">{hint}</p> }
                })}
            <div class="mt-4 overflow-hidden rounded-2xl border border-line bg-surface">
                {children()}
            </div>
        </section>
    }
}

#[component]
pub fn Empty(children: Children) -> impl IntoView {
    view! { <p class="px-5 py-8 text-center text-sm text-faint">{children()}</p> }
}

/// The dot colour of a payment's tier. Classes are literals so Tailwind's
/// scan of `src` emits them.
fn tier_dot(name: &str) -> &'static str {
    match name {
        "human" => "bg-fg",
        "important" => "bg-good",
        "commercial" => "bg-warn",
        "dangerous" => "bg-bad",
        _ => "bg-line-strong",
    }
}

/// A payment's tier: a coloured dot and its lowercased name. A tier the page
/// has no colour for still shows, with the neutral dot.
#[component]
pub fn Tier(#[prop(into)] tier: String) -> impl IntoView {
    let name = tier.to_lowercase();
    view! {
        <span class="flex shrink-0 items-center gap-2">
            <span class=format!("h-2 w-2 rounded-full {}", tier_dot(&name)) />
            <span class="w-20 text-xs text-faint">{name}</span>
        </span>
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTone {
    Good,
    Bad,
    Quiet,
}

impl TagTone {
    fn classes(self) -> &'static str {
        match self {
            Self::Good => "bg-good-soft text-good",
            Self::Bad => "bg-bad-soft text-bad",
            Self::Quiet => "bg-bg text-faint",
        }
    }
}

#[component]
pub fn Tag(tone: TagTone, children: Children) -> impl IntoView {
    view! {
        <span class=format!("rounded-full px-2 py-0.5 text-[11px] {}", tone.classes())>
            {children()}
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_tier_has_its_own_dot_and_the_rest_a_neutral_one() {
        let dots = ["human", "important", "commercial", "dangerous"].map(tier_dot);
        for (index, dot) in dots.iter().enumerate() {
            assert_ne!(*dot, "bg-line-strong");
            assert!(!dots[..index].contains(dot), "{dot} repeats");
        }
        assert_eq!(tier_dot("2"), "bg-line-strong");
    }

    #[test]
    fn every_tag_tone_has_its_own_palette() {
        let tones = [TagTone::Good, TagTone::Bad, TagTone::Quiet].map(TagTone::classes);
        assert!(tones[0] != tones[1] && tones[1] != tones[2] && tones[0] != tones[2]);
    }
}
