# Golden vectors

Byte-exact outputs of the former TypeScript implementation, for testing the Rust
ports. They were captured at commit `eb36792` and are frozen reference data;
there is no command to regenerate them.

The capture called the real functions in `web/src/lib` and the real
`@worldcoin/idkit` / `@worldcoin/idkit-server` packages.

## Fixed inputs

All secrets are throwaway values made for these fixtures. No environment or
`.env` file is read.

| Input | Value |
|---|---|
| `CLASSIFIER_PRIVATE_KEY` | `0x` + `11` x 32 |
| `MESSAGE_ID_SECRET` | `golden-vector-message-id-secret-not-for-production` (raw UTF-8 bytes) |
| World RP signing key | `0x` + `22` x 32 |
| Clock | `Date.now` is replaced; each case records the `now` (seconds) it ran at |
| Randomness | `crypto.randomBytes`, `crypto.randomInt` and `crypto.getRandomValues` are replaced with queued values; each case records the bytes it used |

## Files

| File | Function |
|---|---|
| `sign-quote.json` | `signQuote` (`lib/quote.ts`), EIP-712 `Quote` signature |
| `message-id.json` | `messageIdFor` (`lib/quote.ts`), HMAC-SHA256 |
| `wallet-nonce.json` | `mintWalletNonce` / `verifyWalletNonce` (`lib/wallet-nonce.ts`) |
| `code-hash.json` | `hashCode` / `codeMatches` / `generateCode` (`lib/verification.ts`) |
| `hash-signal.json` | `hashSignal` from `@worldcoin/idkit/hashing` |
| `rp-sign-request.json` | `signRequest` from `@worldcoin/idkit-server` |
| `challenge-email.json` | `challengeMail` (`lib/challenge-email.ts`), subject, html and text |
| `pricing.json` | `quote` (`lib/pricing.ts`) |

Each file has `cases` (or one list per function) of `{ name, input, output }`.
Amounts and floors are decimal strings of 18-decimal USDC base units. A
`verifyWalletNonce` output of `null` means the nonce is refused.
