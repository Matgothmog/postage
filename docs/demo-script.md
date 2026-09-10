# Postage — demo video shot script

One solo author, one recording session, one email's story told twice — once as a
machine, once as a person. Follow this document top to bottom. Nothing in it
requires a decision while filming.

> **Before linking this file publicly:** the Pre-flight and "WOW moment"
> sections below narrate how to engineer the judges' reaction on camera —
> useful to the person recording, but it is stagecraft shown to an audience,
> not project documentation. `README.md` and `SUBMISSION.md` currently link
> here. Recommend this file is *not* linked from the public README; whoever
> does the final pass on that file should make that call deliberately rather
> than by leaving the link in by default.

## The rules (quoted, automatic, non-negotiable)

Check every box before you export. Any one of these failing means automatic
rejection — there is no partial credit.

- [ ] "Must be between 2 and 4 minutes"
- [ ] "DO NOT export the video in any resolution less than 720p"
- [ ] "DO NOT use mobile phones to record the video submission"
- [ ] "DO NOT use a text to speech synthesizer / AI Voiceover"
- [ ] "DO NOT speed up the video to fit under the time limit"
- [ ] "DO NOT play music with text on the video describing your project"
- [ ] "Avoid background noise and echo"

### Export settings

- **Resolution:** 1920×1080 (1080p). This is comfortably above the 720p floor,
  so a small export mistake still clears the bar.
- **Frame rate:** 30 fps, standard screen-recording rate — no reason to deviate.
- **Codec / container:** H.264 in an MP4. Universally playable, no surprises
  on upload.
- **Target runtime: 3:40 (220 seconds).** That is 20 seconds of headroom below
  the 4:00 ceiling and 100 seconds above the 2:00 floor — room for narration
  to run a little long in either direction without crossing either line. Do
  not speed up the final cut to hit this number; if a dry run comes in over
  4:00, cut a shot from the list below, don't accelerate the footage.

## Timing budget

Ten beats, 220 seconds total, 20 seconds of headroom under the 4:00 cap.

| # | Beat | Duration | Environment | What's on screen |
|---|---|---|---|---|
| 1 | Cold open | 15s | Production | Landing page, the "what happens to a message" table |
| 2 | Held + machine-test question | 33s | Production | Send test email; in-thread reply with two links arrives |
| 3 | Machine path — Privy pays | 35s | Production | Embedded wallet created, small USDC payment confirms |
| 4 | Byte-for-byte release | 17s | Production | Forwarded mail arrives with sender's original DKIM intact |
| 5 | Human-test send + handoff | 20s | Production → Preview | Second test email held; same challenge opened on preview |
| 6 | **World ID Selfie Check** (WOW) | 40s | Preview (sandbox) + mirrored iPhone | Face scan on phone, "Verified" screen |
| 7 | Cleared free | 10s | Preview | Challenge page shows released, no charge |
| 8 | The Graph ledger | 30s | Production, `/network` | Settled payment, split, verified count, sender rows |
| 9 | Onchain close | 15s | Production | Arc explorer on the escrow contract, subgraph URL |
| 10 | End card | 5s | N/A (static) | Project name, repo link, sponsor names |
| | **Total** | **220s (3:40)** | | |

## Story framing — why there are two test emails

The product has two outcomes for a held message: a person clears it free, or a
machine pays for it. One email can only take one of those paths, so this
script sends **two** test emails from **two** throwaway sender addresses
during the same recording session — one plays the machine, one plays the
person. This is decided; do not try to make one email do both.

Both throwaway addresses must be ordinary mailboxes at a major provider
(Gmail, Outlook, iCloud — whichever you already have) created fresh for this
recording. **Do not use a disposable/temp-mail service.** Postage's
classifier reads SPF/DKIM/DMARC, and a disposable-mail domain is exactly the
kind of signal that can tip a message into the `dangerous` tier and break the
demo on camera.

## Pre-flight setup

Everything below must be true and verified *before* you press record on the
real take.

1. **Build the preview deploy.** In the Vercel project settings, set
   `NEXT_PUBLIC_WORLD_ENVIRONMENT=sandbox` and `IDENTITY_MODE=live` scoped to
   the **Preview** environment (not Production). Trigger a fresh preview
   deployment (push to a non-`main` branch, or run a manual preview deploy).
   Open the resulting preview URL once — this only confirms the build
   succeeded, nothing about the World ID env var. There is no page you can
   load to confirm sandbox mode is active: the one build-time guard in this
   repo (`web/next.config.ts:23-36`) only fires when `VERCEL_ENV ===
   "production"`, so it never runs on a preview build at all, and
   `NEXT_PUBLIC_WORLD_ENVIRONMENT` itself is only read once you reach the
   Selfie Check call (`web/src/lib/world-id.ts:255`) — a normal page load
   never touches it. To actually confirm this preview is in sandbox mode
   before the take: open the challenge page on the preview URL, trigger the
   World ID verify prompt, and — without tapping or scanning it — inspect
   where the "Continue in World App" connector link points (on iOS Safari,
   long-press the link to preview its destination; on desktop, right-click →
   copy link address). It must resolve to a `sandbox.world.org` host; if it
   resolves to plain `world.org` instead, the env var did not take effect —
   fix it and re-check before recording. Inspecting the link this way
   doesn't submit anything, so it costs neither sandbox account any of its
   one-shot verification budget and is safe to repeat.
2. **Stage two World ID sandbox accounts on the iPhone.** Install World App,
   sign in to the account you intend to film with, and confirm it has not
   yet completed a Selfie Check against this project's sandbox action (its
   `max_verifications` budget is 1 — one successful verification is all it
   gets, ever). Sign in a **second** account on the same phone (or a second
   phone) as a backup. Do not run a real Selfie Check on either account
   before the take — that is what Shot 9 (the Selfie Check take) below is
   for, and it must not happen twice.
3. **Create two fresh throwaway sender mailboxes.** One for the machine-path
   test, one for the human-path test, at a major provider, neither of which
   has ever emailed `you@usepostage.com` before. Confirm this by checking
   that neither address appears yet among the sender rows on
   `postage-seven.vercel.app/network`.
4. **Pin the demo inbox's floor price to one cent.** Sign in as
   `you@usepostage.com`'s own account on the landing page to open its inbox
   panel. It reads `floorPrice` and `effectiveFloor` straight off the chain
   and shows whether a floor was ever chosen (`web/src/app/InboxPanel.tsx:22`
   — `chosen`). If `chosen` is false, the inbox is already on
   `DEFAULT_FLOOR` (`contracts/src/PostageEscrow.sol:41`) — one cent — and
   needs no change. If `chosen` is true, read the floor shown: if it isn't
   $0.01, use the panel's price field to call `setFloorPrice` with `0.01`
   and confirm the panel now reads $0.01. Either way, write down the number
   you verified here — it is what Shot 5's narration must say, not an
   assumption.
5. **Build the mirroring shot.** Connect the iPhone to a Mac by cable. Open
   QuickTime Player → File → New Movie Recording → click the arrow next to
   the record button → select the iPhone as the camera source. This opens a
   live window on the Mac desktop showing the phone's screen. Arrange your
   browser window and this mirror window side by side, both fully visible.
   Mute the mirror window's own audio playback so the phone's sound isn't
   captured twice through your microphone — that doubling is exactly the
   echo the event rules warn against. Record the whole desktop (both windows
   together) with a screen recorder — OBS Studio (Display Capture) or
   QuickTime's own New Screen Recording — with an external or built-in
   microphone as the single audio source.
   - **No Mac available? Fallback:** mount a separate camera (a webcam or a
     standalone camera on a small tripod — never a second phone) pointed
     directly at the iPhone screen. Max out the phone's brightness, dim
     other light sources to kill glare, and feed that camera into the same
     recording setup as a second video source alongside the desktop capture.
6. **Silence notifications everywhere.** Turn on Do Not Disturb / Focus mode
   on both the Mac and the iPhone. Close every app and browser tab you are
   not using in a shot. Nothing should be able to pop a banner across the
   recording.
7. **Set browser zoom for legibility.** Zoom every page you will record
   (landing page, challenge page, `/network`, the explorer) to 125–150%
   before that shot. Numbers and table text must be readable once the final
   export is viewed at 720p on a laptop screen.
8. **Dry run — full run-through, no real verification spent.** Walk every
   shot in order: open each tab, frame the mirror window, time your
   narration against the durations in the timing budget above. For the
   phone portion, use the **backup** World ID account and stop right before
   the final scan step — get the framing and cadence right without actually
   submitting a Selfie Check. Adjust pacing if the dry run runs long, but do
   not compress it by talking faster; cut a shot instead if you must.
   Before you finish the dry run, write down the current "People verified"
   and sender-rows numbers from `/network` — that's the baseline Shot 11
   compares against after the real machine-path payment and Selfie Check in
   the take.

## The shot list

Each shot is its own take. Cut between shots in the edit rather than trying
to record the whole thing in one unbroken pass — this is also your safety
net if any single shot needs a reshoot (see Fallbacks).

---

**Shot 1 — 15s — Environment: Production (`postage-seven.vercel.app`)**
On screen: browser open on the Postage landing page; cursor scrolls to the
"what happens to a message" table.
Narration: *"Marketing mail is free to send you, and free to read too. Not
anymore."*
Counts if: the landing page is fully loaded and the table is legible on
screen for at least 3 seconds.

**Shot 2 — 15s — Environment: Production**
On screen: switch to the machine-test throwaway mailbox; compose a short
email to `you@usepostage.com`; press send.
Narration: *"I'll email this address from an account it's never seen
before."*
Counts if: the send confirmation (message leaves the outbox) is visible on
screen.

**Shot 3 — 18s — Environment: Production**
On screen: the throwaway mailbox receives the in-thread auto-reply; open it;
both links ("a person wrote this" / "a machine wrote this") are visible.
Narration: *"Postage holds it and asks one question, right in the same
thread: person, or machine?"*
Counts if: the reply is visibly threaded to the original message, and both
links are legible on screen.

**Shot 4 — 20s — Environment: Production**
On screen: click "a machine wrote this"; land on the challenge page; start
the Privy sign-in (email or passkey); an embedded wallet address appears.
Narration: *"Say a machine wrote it, and it pays instead. No wallet, no seed
phrase — just an email."*
Counts if: a wallet address appears on screen without any separate
wallet-connect step.

**Shot 5 — 15s — Environment: Production**
On screen: confirm the small USDC payment; the page moves to a "paid /
released" state.
Narration: *"It pays one cent in USDC — the same coin that pays for its
own gas on Arc."*
Counts if: an on-screen confirmation (success state or transaction
reference) is visible after the payment step.

**Shot 6 — 17s — Environment: Production**
On screen: switch to the real recipient inbox; the forwarded message has
arrived; open the message headers or the mail client's own
signature-verified indicator.
Narration: *"The email that lands is the exact one that was sent — even the
sender's own signature still checks out."*
Counts if: the forwarded message is visible with a signature-pass indicator
(or headers showing the original DKIM domain) on screen.

**Shot 7 — 10s — Environment: Production**
On screen: from the human-test throwaway mailbox, send a second short email
to `you@usepostage.com`; cut to its in-thread reply arriving.
Narration: *"Now the same question, from someone who actually is a person."*
Counts if: the second reply is visible in its own thread with both links
legible.

**Shot 8 — 10s — Environment: Production → Preview handoff**
On screen: cursor highlights the link text after "a person wrote this" —
everything after the domain. Do not click it. Open a new tab, paste the
preview deployment's URL, and append that same path. The preview page loads.
Narration: *"This time I open it on our sandbox build, where World ID's real
Selfie Check runs."*
Counts if: the address bar clearly shows the preview URL (not
`postage-seven.vercel.app`), and the same challenge page loads there.

**Shot 9 — 40s — Environment: Preview (sandbox) + mirrored iPhone — THE WOW MOMENT**
On screen: the preview page shows the World ID verify prompt; cut to the
mirrored iPhone window — World App is open on the sandbox action; the Selfie
Check camera opens, the scan completes, the app shows "Verified." Hold this
frame an extra beat before cutting away (see WOW moment section below).
Narration: *"A quick face scan in World's sandbox app proves I'm a real
person — nothing else."*
Counts if: the mirrored phone window shows the World App's "Verified"
success screen, legible in the recording, using the primary account's one
available verification.

**Shot 10 — 10s — Environment: Preview**
On screen: back on the preview browser tab, the challenge page updates to
show the message cleared with no payment.
Narration: *"Back on the page, it's cleared. No charge, no wallet, nothing to
sign."*
Counts if: the on-screen state reads as released/cleared with no payment
step shown.

**Shot 11 — 30s — Environment: Production, `/network`**
On screen: navigate to `postage-seven.vercel.app/network`; cursor highlights
in turn: a settled payment row, the "People verified" count, and the sender
rows count.
Narration: *"Every payment and every verification is public — this page
reads it straight off the chain, through The Graph."*
Counts if: a settled payment row is visible in the recent-payments list (it
does not have to be from this session — see Fallbacks if indexing is
lagging); the "People verified" count reads at least one higher than the
baseline number you wrote down at the end of the pre-flight dry run
(step 8); and the sender-rows count also reads at least one higher than
that same baseline. Do not require the $0.024/$0.006 split to be
visible — `/network` never displays a per-payment split, only the
aggregate "Paid to inboxes" total (`web/src/app/network/page.tsx:73`). Hold
each figure long enough to read, roughly 5–6 seconds apiece.

**Shot 12 — 15s — Environment: Production**
On screen: open a new tab to the Arc testnet explorer at
`testnet.arcscan.app` on the PostageEscrow contract
(`0x4469e869433cf6cc08dd54afc6ac7e288b9a38f7`); show the contract and a
transaction; quickly show the subgraph Studio query URL
(`https://api.studio.thegraph.com/query/1758667/usepostage/v0.4.0`).
Narration: *"The contracts, the payment, the attestation — all of it lives
onchain on Arc, indexed and queryable."*
Counts if: the explorer page is loaded showing the contract address and at
least one transaction.

**Shot 13 — 5s — Environment: N/A (static end card)**
On screen: text card — project name, repo link
(`github.com/Matgothmog/postage`), and the sponsor names built on (Arc,
World ID, The Graph, Privy).
Narration: *"That's Postage — mail that pays for itself."*
Counts if: the card is fully legible and holds for its full 5 seconds with
no motion needed.

---

## The WOW moment

**Shot 9 — the Selfie Check clearing free — is the one a judge remembers.**
It needs no architecture explained to land: a face unlocks someone's mail,
for free, with no wallet and no gas, in front of the camera. That is
instantly legible to a judge who has never read a line of the code, and it
demonstrates the World ID beat at full strength in the same breath.

Give it the extra beat by holding on the phone's "Verified" screen for a
full two-count after the checkmark appears, in silence, before cutting to
Shot 10 — do not talk over it and do not cut away the instant it appears.
Let the moment sit.

(The byte-for-byte DKIM-preserving release in Shot 6 is the more
technically impressive fact in the whole project, but it requires a judge to
already understand what DKIM is to feel the payoff — it is real technical
depth, not a wow. Selfie Check wins the wow slot; Shot 6 still earns its own
17 seconds for the judges scoring Technicality.)

## Fallbacks

Every shot is its own take (see "The shot list" above), so a single failure
means reshooting one clip, not restarting the session.

- **The Selfie Check proof doesn't complete on the primary account (Shot
  9).** Force-quit and relaunch World App, and retry once on the primary
  account. If it fails again, switch immediately to the backup account
  staged in pre-flight step 2 and reshoot the shot with that account
  instead — the narration line does not need to change.
- **The sandbox action is slow to load on a semi-cold iOS app.** Warm the
  app up during pre-flight: open World App, navigate to the sandbox action's
  verify screen, and background the app (without submitting) a few minutes
  before recording starts, so the take itself isn't the first cold load of
  the session.
- **The subgraph hasn't indexed the new machine-path payment yet by Shot
  11.** Do not pause recording to wait for it. Film `/network` showing
  whatever settled data is currently live and correct — the page already
  shows real indexed history from before this session, so the shot is true
  either way. Keep the narration generic ("a settled payment," not "the
  payment we just made") so nothing on screen contradicts what's said. The
  same lag can hit the "People verified" and sender-rows counts in Shot 11 —
  if either still reads at the pre-flight baseline instead of one higher,
  don't wait for it either: film whatever the page shows right now and let
  the narration stay on "every payment and every verification is public"
  rather than pointing at a specific number changing on screen.
- **A throwaway mailbox's message gets misclassified or delayed.** Have a
  third backup throwaway address ready at each provider. Resend from the
  backup and reshoot only the shots downstream of the failure.

## Post-production

**Permitted:** cutting between shots, captions/subtitles burned in or as a
track, still frames (for example holding Shot 9's "Verified" screen or the
Shot 13 end card).

**Not permitted:** speeding up any footage to fit the time limit, adding an
AI-generated or text-to-speech voiceover over any of it, and playing music
under any shot that has on-screen text explaining the project (which, given
the end card and any captions, effectively means: do not add a music track
to this edit at all — narration and natural screen-recording audio only).
