/// Palette for `challenge-email.ts`. The email is a runtime-built HTML
/// string sent through Resend — it never sees `globals.css`, so before this
/// file existed its colors were ~20 hex literals typed straight into the
/// markup, a silent duplicate of the app palette with no way to tell when
/// the two drifted.
///
/// This is deliberately a LIGHT palette, not the app's dark electric-blue
/// theme (`globals.css`): Gmail on Android inverts dark email bodies, some
/// Outlook builds drop CSS backgrounds on elements with no `bgcolor`
/// attribute, and this repo has no email-preview tooling to check a dark
/// build against either client. So the email mirrors the app's tokens at a
/// lighter tint instead of reusing them outright — the two palettes are
/// meant to move together, not stay pixel-identical.
///
/// `accent` below is `#1D4ED8`, matching the app's `--accent-lo`
/// (`globals.css`), not `--accent` (`#3B82F6`): on white, `#3B82F6` is
/// 3.68:1 and fails WCAG AA for text, while `#1D4ED8` is 6.3:1, and white
/// text on an `#1D4ED8` fill is 6.3:1 too. If the app palette in
/// `globals.css` moves, re-check these against it by hand — nothing wires
/// them together automatically.
export const bg = "#F5F8FF" as const; // page background
export const card = "#FFFFFF" as const; // the message panel
export const ink = "#0F172A" as const; // body text
export const inkSoft = "#475569" as const; // secondary text
export const accent = "#1D4ED8" as const; // buttons, links
export const line = "#DBE4F0" as const; // rules and borders
export const onAccent = "#FFFFFF" as const; // label text on an accent-filled button
