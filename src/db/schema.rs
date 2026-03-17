//! SQLite table definitions and migrations.
//!
//! Call [`run_migrations`] once on startup — it is safe to call on an existing
//! database (all statements use `CREATE TABLE IF NOT EXISTS`).
//!
//! # Schema overview
//!
//! ```
//! swaps
//! ├── id               INTEGER PRIMARY KEY AUTOINCREMENT
//! ├── pool_address     TEXT    NOT NULL          -- 0x-hex address
//! ├── block_number     INTEGER NOT NULL
//! ├── block_timestamp  INTEGER NOT NULL          -- Unix seconds
//! ├── tx_hash          TEXT    NOT NULL          -- 0x-hex bytes32
//! ├── log_index        INTEGER NOT NULL
//! ├── sender           TEXT    NOT NULL
//! ├── recipient        TEXT    NOT NULL
//! ├── amount0          TEXT    NOT NULL          -- signed decimal i128
//! ├── amount1          TEXT    NOT NULL          -- signed decimal i128
//! ├── sqrt_price_x96   TEXT    NOT NULL          -- 0x-hex U256
//! ├── liquidity        TEXT    NOT NULL          -- 0x-hex u128
//! └── tick             INTEGER NOT NULL          -- i32
//!
//! liquidity_changes
//! ├── id               INTEGER PRIMARY KEY AUTOINCREMENT
//! ├── event_type       TEXT    NOT NULL          -- 'mint' | 'burn'
//! ├── pool_address     TEXT    NOT NULL
//! ├── block_number     INTEGER NOT NULL
//! ├── block_timestamp  INTEGER NOT NULL
//! ├── tx_hash          TEXT    NOT NULL
//! ├── log_index        INTEGER NOT NULL
//! ├── owner            TEXT    NOT NULL
//! ├── sender           TEXT                      -- NULL for burns
//! ├── tick_lower       INTEGER NOT NULL
//! ├── tick_upper       INTEGER NOT NULL
//! ├── amount           TEXT    NOT NULL          -- 0x-hex u128 (liquidity units)
//! ├── amount0          TEXT    NOT NULL          -- 0x-hex u128
//! └── amount1          TEXT    NOT NULL          -- 0x-hex u128
//! ```

/// SQL statements executed in order on every startup.
///
/// Each statement is idempotent (`IF NOT EXISTS` / `IF NOT EXISTS`).
pub const MIGRATIONS: &[&str] = &[
    // ── swaps ────────────────────────────────────────────────────────────
    "
    CREATE TABLE IF NOT EXISTS swaps (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,

        -- Pool identity
        pool_address     TEXT    NOT NULL,

        -- Block context
        block_number     INTEGER NOT NULL,
        block_timestamp  INTEGER NOT NULL,   -- Unix epoch seconds

        -- Transaction identity
        tx_hash          TEXT    NOT NULL,
        log_index        INTEGER NOT NULL,   -- position within the tx

        -- Participants
        sender           TEXT    NOT NULL,   -- caller of pool.swap()
        recipient        TEXT    NOT NULL,   -- receiver of output tokens

        -- Token deltas (signed decimal strings to handle i128)
        -- Positive  → tokens flowed INTO the pool
        -- Negative  → tokens flowed OUT of the pool
        amount0          TEXT    NOT NULL,
        amount1          TEXT    NOT NULL,

        -- Post-swap pool state — sufficient to reconstruct current price
        sqrt_price_x96   TEXT    NOT NULL,   -- 0x-hex U256
        liquidity        TEXT    NOT NULL,   -- 0x-hex u128; active range liquidity
        tick             INTEGER NOT NULL,   -- i32; current tick after swap

        -- Deduplication: a given log occupies exactly one position in a tx
        UNIQUE (tx_hash, log_index)
    );
    ",
    // ── swaps indices ─────────────────────────────────────────────────────
    //
    // Historical sync walks forward by block; live sync filters by pool.
    // Both access patterns are covered by these two indices.
    "
    CREATE INDEX IF NOT EXISTS idx_swaps_pool_block
        ON swaps (pool_address, block_number);
    ",
    // ── liquidity_changes ────────────────────────────────────────────────
    "
    CREATE TABLE IF NOT EXISTS liquidity_changes (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,

        -- Discriminator: 'mint' | 'burn'
        event_type       TEXT    NOT NULL CHECK (event_type IN ('mint', 'burn')),

        -- Pool identity
        pool_address     TEXT    NOT NULL,

        -- Block context
        block_number     INTEGER NOT NULL,
        block_timestamp  INTEGER NOT NULL,

        -- Transaction identity
        tx_hash          TEXT    NOT NULL,
        log_index        INTEGER NOT NULL,

        -- Position owner (both events); the address that holds the LP position
        owner            TEXT    NOT NULL,

        -- Mint-only: the address that initiated the mint (may differ from owner
        -- when a router/manager contract is used).  NULL for burns.
        sender           TEXT,

        -- Position tick bounds
        tick_lower       INTEGER NOT NULL,
        tick_upper       INTEGER NOT NULL,

        -- Liquidity units added (mint) or removed (burn) — 0x-hex u128.
        -- This is raw V3 liquidity, NOT a token amount.
        amount           TEXT    NOT NULL,

        -- Actual token amounts moved — 0x-hex u128; always non-negative
        amount0          TEXT    NOT NULL,
        amount1          TEXT    NOT NULL,

        -- Deduplication
        UNIQUE (tx_hash, log_index)
    );
    ",
    // ── liquidity_changes indices ─────────────────────────────────────────
    //
    // Reconstructing the liquidity distribution across all ticks for a given
    // pool requires scanning every row for that pool.  A pool+block index
    // supports both historical replay and incremental updates.
    "
    CREATE INDEX IF NOT EXISTS idx_lc_pool_block
        ON liquidity_changes (pool_address, block_number);
    ",
    // Reconstructing a *specific* position (owner + tick range) is a common
    // query when checking whether a position still has liquidity.
    "
    CREATE INDEX IF NOT EXISTS idx_lc_pool_owner_ticks
        ON liquidity_changes (pool_address, owner, tick_lower, tick_upper);
    ",
];

/// Run all pending migrations against `conn`.
///
/// Executes each statement in [`MIGRATIONS`] in order.  Statements are
/// idempotent, so this is safe to call on startup regardless of whether the
/// database already exists.
///
/// # Errors
///
/// Returns an [`rusqlite::Error`] if any statement fails.  A failure mid-way
/// leaves the database in a partially migrated state; callers should treat any
/// error here as fatal and refuse to start.
pub fn run_migrations(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    for sql in MIGRATIONS {
        conn.execute_batch(sql)?;
    }
    Ok(())
}
