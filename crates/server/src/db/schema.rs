//! The shape of the database, applied on every cold start. `IF NOT EXISTS`
//! throughout, so this is the whole story only for a database created today -
//! see [`super::migrations`] for what an older one is missing.
//!
//! Every DDL string here is byte-identical to `web/src/lib/db/schema.ts`, which
//! created the live Turso database; a test holds them to that. The live
//! database also has an `allowlist` table that no SQL in the repo describes.
//! Nothing here names it, and nothing may: bootstrap only ever creates what is
//! missing, so a table it does not know about is left exactly as it is.

/// One statement of the schema, tagged with the bootstrap phase it belongs to.
///
/// The TypeScript derived the phase by asking whether the text starts with
/// `CREATE TABLE`, and had to explain at length which spellings that misreads
/// (lower case, `CREATE VIRTUAL TABLE`) and why the misreading is costly. Here
/// the phase is written down beside each statement instead of guessed from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaStatement {
    /// Makes a table exist. Runs before the migration, which can only alter a
    /// table that is already there.
    CreateTable(&'static str),
    /// May name a column on a table - today every index. Runs after the
    /// migration, which is what adds that column to a database made before it
    /// existed.
    ColumnDependent(&'static str),
}

impl SchemaStatement {
    pub const fn sql(self) -> &'static str {
        match self {
            Self::CreateTable(sql) | Self::ColumnDependent(sql) => sql,
        }
    }
}

use SchemaStatement::{ColumnDependent, CreateTable};

/// The rule the phase split does *not* enforce, and the one the next person
/// actually needs: **a new indexed column is two edits, not one.** Adding it to
/// the `CREATE TABLE` here is the shape a database made today gets; every
/// database made before today gets it only from an `ADDED_COLUMNS` entry in
/// [`super::migrations`]. Without that entry an index names a column half the
/// world is missing, and running late does not save it - which is exactly how
/// `claim_sends (wallet, sent_at)` took the gateway down.
pub const SCHEMA: [SchemaStatement; 21] = [
    CreateTable(
        "CREATE TABLE IF NOT EXISTS inboxes (
     handle TEXT PRIMARY KEY,
     destination TEXT NOT NULL,
     wallet TEXT,
     created_at INTEGER NOT NULL
   )",
    ),
    ColumnDependent("CREATE INDEX IF NOT EXISTS inboxes_by_wallet ON inboxes (wallet)"),
    // A sender's permission to reach an inbox, and it runs out. Proving
    // personhood buys a short window rather than a standing welcome, because
    // the proof says a person was there a moment ago, not that this address
    // belongs to one. Paying buys exactly one delivery.
    //
    // `paid_extended_at` is set the moment a payment stretches an
    // already-unlimited window rather than buying a count - the one case where
    // money lands on a row and `uses_left` stays NULL regardless, since there
    // is no count to add a use to. It is what tells a window nobody ever paid
    // for from one that has, permanently, once set.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS passes (
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     reason TEXT NOT NULL,
     expires_at INTEGER NOT NULL,
     uses_left INTEGER,
     created_at INTEGER NOT NULL,
     paid_extended_at INTEGER,
     PRIMARY KEY (handle, sender)
   )",
    ),
    // A handle someone is part way through claiming. It becomes a row in
    // `inboxes` only once they have proved they can read the address they are
    // pointing it at, so an unfinished claim forwards nothing.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS inbox_claims (
     handle TEXT PRIMARY KEY,
     destination TEXT NOT NULL,
     wallet TEXT NOT NULL,
     code_hash TEXT NOT NULL,
     expires_at INTEGER NOT NULL,
     attempts INTEGER NOT NULL DEFAULT 0,
     code_verified_at INTEGER,
     cf_address_id TEXT,
     cf_verified_at INTEGER,
     cf_checked_at INTEGER,
     cf_checks INTEGER NOT NULL DEFAULT 0,
     created_at INTEGER NOT NULL
   )",
    ),
    // One row per message we paid a model to read. Kept only long enough to cap
    // what a flood can cost.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS classifications (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     at INTEGER NOT NULL
   )",
    ),
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS classifications_recent ON classifications (handle, at)",
    ),
    // The purge filters on age alone, which the composite index above cannot
    // answer without reading the table.
    ColumnDependent("CREATE INDEX IF NOT EXISTS classifications_by_age ON classifications (at)"),
    // One row per claim anyone has started, kept only long enough to throttle
    // on. `inbox_claims` cannot answer either question it exists for: it holds
    // one row per handle, so it cannot say how often a mailbox has been asked
    // to confirm, and it loses the row entirely once the claim goes live.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS claim_sends (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     destination TEXT NOT NULL,
     wallet TEXT,
     sent_at INTEGER NOT NULL
   )",
    ),
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS claim_sends_by_destination ON claim_sends (destination, sent_at)",
    ),
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS claim_sends_by_wallet ON claim_sends (wallet, sent_at)",
    ),
    // The purge filters on age alone, which neither index above can answer.
    ColumnDependent("CREATE INDEX IF NOT EXISTS claim_sends_by_age ON claim_sends (sent_at)"),
    // The wallet a sender last paid from, so the next message they write can be
    // priced on what The Graph knows about them rather than as a stranger.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS sender_wallets (
     sender TEXT PRIMARY KEY,
     wallet TEXT NOT NULL,
     linked_at INTEGER NOT NULL
   )",
    ),
    // Which sender each World ID nullifier belongs to *at the moment*.
    //
    // The nullifier names a person, and the primary key is what keeps one
    // person to one free lane: the row cannot name two senders, so proving
    // again under a second address takes the lane off the first rather than
    // opening another. It is not a permanent bond, because the sender a proof
    // binds to is the one on the challenge token rather than the one who took
    // the selfie, and a permanent row would let a mailed link cost a stranger
    // their lane for good. Nothing here identifies anyone: the nullifier is
    // scoped to us and means nothing anywhere else.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS nullifiers (
     nullifier_hash TEXT PRIMARY KEY,
     sender TEXT NOT NULL,
     claimed_at INTEGER NOT NULL
   )",
    ),
    // Every time a nullifier's binding moved from one sender to another.
    //
    // The row above holds only the current answer, and overwriting it is
    // exactly the operation worth being able to look back at: a lane changing
    // hands is either somebody recovering from a poisoned link or somebody
    // working the policy, and neither is visible from a table that only ever
    // states today. Kept in its own table rather than as columns on the row so
    // that the whole chain survives, not just the last hop.
    //
    // Not purged, unlike the other append-only tables here. One row per
    // takeover is bounded by how often bindings actually move, and volume here
    // is the abuse signal - deleting it would erase the only reason to keep it.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS nullifier_rebinds (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     nullifier_hash TEXT NOT NULL,
     from_sender TEXT NOT NULL,
     to_sender TEXT NOT NULL,
     at INTEGER NOT NULL
   )",
    ),
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS nullifier_rebinds_by_nullifier ON nullifier_rebinds (nullifier_hash, id)",
    ),
    // A message being held, and the terms for releasing it.
    //
    // Nothing anyone wrote is in this table. The message itself is held by the
    // worker that received it, so that releasing it can put the original bytes
    // on the wire; `held_until` is only this side's record that it still
    // exists.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS challenges (
     token TEXT PRIMARY KEY,
     handle TEXT NOT NULL,
     sender TEXT NOT NULL,
     message_id TEXT NOT NULL,
     tier TEXT NOT NULL,
     amount TEXT NOT NULL,
     quote_json TEXT NOT NULL,
     held_until INTEGER,
     created_at INTEGER NOT NULL,
     resolved_at INTEGER,
     entitled_at INTEGER,
     delivered_at INTEGER,
     settled_by TEXT
   )",
    ),
    // Every rp_context we have signed, against the challenge it was signed for.
    //
    // A signed context is a bearer credential naming this relying party and
    // this action. Keeping it is what caps how many one challenge may mint and
    // what lets a proof coming back be checked against a request we actually
    // made.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS issued_rp_contexts (
     nonce TEXT PRIMARY KEY,
     token TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     expires_at INTEGER NOT NULL,
     consumed_at INTEGER
   )",
    ),
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS issued_rp_contexts_by_token ON issued_rp_contexts (token, expires_at)",
    ),
    // The purge filters on the window's end alone, which the composite index
    // above cannot answer without reading the table.
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS issued_rp_contexts_by_age ON issued_rp_contexts (expires_at)",
    ),
    // Every wallet nonce that has already been answered with a valid signature.
    //
    // Deliberately not the mirror of `issued_rp_contexts` above: nothing is
    // written here when a nonce is *handed out*. `/api/wallet-nonce` is open,
    // so a row per issue would be a table any stranger could grow without
    // limit. A minted nonce carries its own MAC instead, and only a nonce that
    // has already survived signature verification ever reaches this table.
    //
    // The primary key is the whole mechanism: the second presentation of one
    // signature finds the row already there and is refused. `expires_at` is
    // carried so the row can be dropped once the nonce would be refused on age
    // anyway, which is what keeps the table the size of one window.
    CreateTable(
        "CREATE TABLE IF NOT EXISTS spent_wallet_nonces (
     nonce TEXT PRIMARY KEY,
     expires_at INTEGER NOT NULL
   )",
    ),
    // The purge filters on the window's end alone, and the primary key above
    // cannot answer that without reading the table.
    ColumnDependent(
        "CREATE INDEX IF NOT EXISTS spent_wallet_nonces_by_age ON spent_wallet_nonces (expires_at)",
    ),
];

/// The statements that make a table exist, in schema order. Bootstrap runs
/// these first.
pub fn table_statements() -> impl Iterator<Item = &'static str> {
    SCHEMA.into_iter().filter_map(|statement| match statement {
        CreateTable(sql) => Some(sql),
        ColumnDependent(_) => None,
    })
}

/// Everything else, in schema order. Bootstrap runs these after the migration.
pub fn column_dependent_statements() -> impl Iterator<Item = &'static str> {
    SCHEMA.into_iter().filter_map(|statement| match statement {
        CreateTable(_) => None,
        ColumnDependent(sql) => Some(sql),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `web/src/lib/db/schema.ts` with:
    /// `node --experimental-strip-types -e 'import("./web/src/lib/db/schema.ts").then(m => ...)'`
    /// serialising `SCHEMA`, `TABLE_STATEMENTS` and `COLUMN_DEPENDENT_STATEMENTS`.
    #[derive(Debug, serde::Deserialize)]
    struct TypeScriptSchema {
        schema: Vec<String>,
        table_statements: Vec<String>,
        column_dependent_statements: Vec<String>,
    }

    fn typescript_schema() -> TypeScriptSchema {
        serde_json::from_str(include_str!("../../tests/fixtures/schema.json")).unwrap()
    }

    #[test]
    fn every_statement_is_byte_identical_to_the_typescript_schema() {
        let expected = typescript_schema().schema;
        let actual: Vec<&str> = SCHEMA.iter().map(|statement| statement.sql()).collect();
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            assert_eq!(actual, expected, "statement {index} differs");
        }
    }

    #[test]
    fn the_phase_split_matches_the_one_the_typescript_derived() {
        let expected = typescript_schema();
        assert_eq!(
            table_statements().collect::<Vec<_>>(),
            expected.table_statements
        );
        assert_eq!(
            column_dependent_statements().collect::<Vec<_>>(),
            expected.column_dependent_statements
        );
    }

    #[test]
    fn every_tagged_table_statement_creates_a_table_and_nothing_else_does() {
        for statement in SCHEMA {
            let creates_table = statement.sql().starts_with("CREATE TABLE IF NOT EXISTS ");
            assert_eq!(
                matches!(statement, CreateTable(_)),
                creates_table,
                "{}",
                statement.sql()
            );
        }
    }

    #[test]
    fn the_schema_never_names_the_allowlist_table() {
        for statement in SCHEMA {
            assert!(
                !statement.sql().contains("allowlist"),
                "{}",
                statement.sql()
            );
        }
    }
}
