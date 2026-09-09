import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { confirmStatement as confirmStatementDirect } from "@/lib/statements";
import { confirmStatement as confirmStatementReExported } from "@/lib/wallet-proof";

/// FinishClaim.tsx itself cannot be imported here - see Account.test.ts for
/// why .tsx is out of reach for this test runner - so fix 3 (importing
/// `confirmStatement` from `@/lib/wallet-proof`, where the header says both
/// ends of the signed-header contract are meant to be read from, rather than
/// from `@/lib/statements` directly) is pinned two ways: that the two names
/// are the same function, and that the source actually reads it from the
/// one wallet-proof.ts names as the intended door.
const FINISH_CLAIM_SOURCE = readFileSync(
  fileURLToPath(new URL("./FinishClaim.tsx", import.meta.url)),
  "utf8"
);

test("wallet-proof re-exports the exact confirmStatement statements.ts defines (fix 3)", () => {
  assert.equal(
    confirmStatementReExported,
    confirmStatementDirect,
    "@/lib/wallet-proof must re-export the same confirmStatement, not a second copy"
  );
});

test("FinishClaim.tsx imports confirmStatement from wallet-proof, not statements directly (fix 3)", () => {
  assert.match(
    FINISH_CLAIM_SOURCE,
    /import\s*\{[^}]*\bconfirmStatement\b[^}]*\}\s*from\s*"@\/lib\/wallet-proof"/,
    "expected an import of confirmStatement from @/lib/wallet-proof"
  );
  assert.doesNotMatch(
    FINISH_CLAIM_SOURCE,
    /import\s*\{[^}]*\bconfirmStatement\b[^}]*\}\s*from\s*"@\/lib\/statements"/,
    "confirmStatement must not be imported from @/lib/statements directly"
  );
});
