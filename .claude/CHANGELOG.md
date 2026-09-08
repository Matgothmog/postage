## 2026-09-08 — refactor/sweep

Review pass over the whole repo. The four tiers and the mail domain each had
several copies that could drift; both are now stated once. The worker believes an
`Authentication-Results` method only when every copy of that header agrees, so a
sender can no longer contradict Cloudflare and be believed. A destination on our
own domain is refused rather than looped, and that refusal — like an inbox with no
wallet — is a verdict with a bounce rather than a 409 that told the sender's
server to retry forever. Cloudflare's destination list is now read past the first
page. `KNOWN_ISSUES` gained one entry it was missing and lost one that was not a
bug.
