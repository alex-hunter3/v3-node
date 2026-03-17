//! Rust structs that map 1-to-1 to SQLite rows.
//!
//! # Type mapping rationale
//!
//! | Solidity / Alloy type | SQLite column type | Rust field type |
//! |-----------------------|--------------------|-----------------|
//! | `address`             | `TEXT`  (0x-hex)   | `String`        |
//! | `bytes32` / tx hash   | `TEXT`  (0x-hex)   | `String`        |
//! | `uint128` / `int128`  | `TEXT`  (0x-hex)   | `String`        |
//! | `uint256`             | `TEXT`  (0x-hex)   | `String`        |
//! | `int24`  (tick)       | `INTEGER`          | `i32`           |
//! | `uint64` (block, ts)  | `INTEGER`          | `i64`           |
//! | `uint32` (log index)  | `INTEGER`          | `i32`           |
//!
//! `u128` / `i128` / `U256` all exceed SQLite's native signed-64-bit
//! `INTEGER`. Hex strings are lossless, unambiguous, and trivially
//! round-tripped with `alloy::primitives`.
//!
//! # Conversion helpers
//!
//! Each model provides `From<&DecodedEvent>` impls (in `pool/events.rs`) that
//! convert alloy primitives into the storage types used here.  The DB layer
//! (`db/queries.rs`) only ever sees these plain structs.

use alloy::primitives::{Address, B256, U256};

// ─── Shared helpers ──────────────────────────────────────────────────────────

/// Encode a `U256` as a lower-case `0x`-prefixed hex string for storage.
pub fn u256_to_hex(v: U256) -> String {
    format!("0x{v:x}")
}

/// Encode an `i128` (signed) as a decimal string for storage.
///
/// Signed amounts can be negative (e.g. `amount0` / `amount1` on a swap), so
/// we use decimal rather than hex to preserve the sign unambiguously.
pub fn i128_to_str(v: i128) -> String {
    v.to_string()
}

/// Encode a `u128` as a lower-case `0x`-prefixed hex string for storage.
pub fn u128_to_hex(v: u128) -> String {
    format!("0x{v:x}")
}

// ─── SwapRow ─────────────────────────────────────────────────────────────────

/// One row in the `swaps` table.
///
/// Corresponds to a single `Swap(address,address,int256,int256,uint160,uint128,int24)`
/// log emitted by a Uniswap V3 pool contract.
///
/// The three fields `sqrt_price_x96`, `liquidity`, and `tick` capture the
/// pool's full AMM state *after* the swap, which is sufficient to reconstruct
/// the current price and active-range liquidity at any historical block without
/// replaying any arithmetic.
#[derive(Debug, Clone)]
pub struct SwapRow {
    // ── Identity ──────────────────────────────────────────────────────────
    /// Ethereum address of the pool contract (0x-hex, lowercase).
    pub pool_address: String,
    /// Block number in which the swap was included.
    pub block_number: i64,
    /// Transaction hash (0x-hex).
    pub tx_hash: String,
    /// Position of this log within the transaction (used for ordering and
    /// deduplication).
    pub log_index: i32,
    /// Unix timestamp of the block.
    pub block_timestamp: i64,

    // ── Participants ──────────────────────────────────────────────────────
    /// Address that initiated the swap (i.e. called `pool.swap()`).
    pub sender: String,
    /// Address that received the output tokens.
    pub recipient: String,

    // ── Token flows ───────────────────────────────────────────────────────
    /// Net change in the pool's token0 reserve (signed decimal string).
    /// Positive  → tokens entered the pool from the swapper.
    /// Negative → tokens left the pool to the recipient.
    pub amount0: String,
    /// Net change in the pool's token1 reserve (signed decimal string).
    pub amount1: String,

    // ── Post-swap pool state ──────────────────────────────────────────────
    /// `sqrtPriceX96` after the swap (0x-hex `U256`).
    /// Together with `tick`, this fully defines the spot price.
    pub sqrt_price_x96: String,
    /// Active liquidity in the current tick range *after* the swap (0x-hex `u128`).
    pub liquidity: String,
    /// Current tick *after* the swap (`i32`, stored as `INTEGER`).
    pub tick: i32,
}

impl SwapRow {
    /// Construct a [`SwapRow`] from decoded event fields.
    ///
    /// * `pool`       – address of the pool contract
    /// * `block`      – block number
    /// * `ts`         – block timestamp (Unix seconds)
    /// * `tx`         – transaction hash
    /// * `log_idx`    – log index within the transaction
    /// * `sender`     – swap initiator
    /// * `recipient`  – output recipient
    /// * `amount0`    – signed token-0 delta (i128)
    /// * `amount1`    – signed token-1 delta (i128)
    /// * `sqrt_price` – post-swap sqrtPriceX96 (U256)
    /// * `liquidity`  – post-swap active liquidity (u128)
    /// * `tick`       – post-swap current tick (i32)
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: Address,
        block: u64,
        ts: u64,
        tx: B256,
        log_idx: u32,
        sender: Address,
        recipient: Address,
        amount0: i128,
        amount1: i128,
        sqrt_price: U256,
        liquidity: u128,
        tick: i32,
    ) -> Self {
        Self {
            pool_address: format!("{pool:?}"),
            block_number: block as i64,
            block_timestamp: ts as i64,
            tx_hash: format!("{tx:?}"),
            log_index: log_idx as i32,
            sender: format!("{sender:?}"),
            recipient: format!("{recipient:?}"),
            amount0: i128_to_str(amount0),
            amount1: i128_to_str(amount1),
            sqrt_price_x96: u256_to_hex(sqrt_price),
            liquidity: u128_to_hex(liquidity),
            tick,
        }
    }
}

// ─── LiquidityChangeKind ─────────────────────────────────────────────────────

/// Discriminates between `Mint` and `Burn` events in the shared
/// `liquidity_changes` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidityChangeKind {
    /// Liquidity was added (`Mint` event).
    Mint,
    /// Liquidity was removed (`Burn` event).
    Burn,
}

impl LiquidityChangeKind {
    /// The string stored in the `event_type` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mint => "mint",
            Self::Burn => "burn",
        }
    }

    /// Parse the `event_type` column back into a [`LiquidityChangeKind`].
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "mint" => Some(Self::Mint),
            "burn" => Some(Self::Burn),
            _ => None,
        }
    }

    /// Sign to apply when summing liquidity for a tick range:
    /// `+1` for mints, `-1` for burns.
    pub fn sign(self) -> i64 {
        match self {
            Self::Mint => 1,
            Self::Burn => -1,
        }
    }
}

// ─── LiquidityChangeRow ──────────────────────────────────────────────────────

/// One row in the `liquidity_changes` table.
///
/// Covers both `Mint(address,address,int24,int24,uint128,uint256,uint256)` and
/// `Burn(address,int24,int24,uint128,uint256,uint256)` events.  They share an
/// identical field shape, so a single table with an `event_type` discriminator
/// avoids duplication and lets tick-level liquidity be reconstructed with a
/// single aggregate query:
///
/// ```sql
/// SELECT
///     tick_lower,
///     tick_upper,
///     SUM(CASE WHEN event_type = 'mint' THEN CAST(amount AS INTEGER)
///              ELSE -CAST(amount AS INTEGER) END) AS net_liquidity
/// FROM liquidity_changes
/// WHERE pool_address = ?
/// GROUP BY tick_lower, tick_upper;
/// ```
///
/// (In practice `amount` is stored as hex and must be decoded in application
/// code before arithmetic; the query above is illustrative.)
#[derive(Debug, Clone)]
pub struct LiquidityChangeRow {
    // ── Identity ──────────────────────────────────────────────────────────
    /// `"mint"` or `"burn"` — see [`LiquidityChangeKind`].
    pub event_type: String,
    /// Ethereum address of the pool contract (0x-hex, lowercase).
    pub pool_address: String,
    /// Block number in which the event was included.
    pub block_number: i64,
    /// Transaction hash (0x-hex).
    pub tx_hash: String,
    /// Position of this log within the transaction.
    pub log_index: i32,
    /// Unix timestamp of the block.
    pub block_timestamp: i64,

    // ── Participants ──────────────────────────────────────────────────────
    /// For `Mint`: address that called the pool's `mint()` function.
    /// For `Burn`: the position owner.
    pub owner: String,
    /// For `Mint`: the address that will receive any fees collected on behalf
    /// of `owner` (the `sender` in the Mint event signature).
    /// `None` for `Burn` events, which have no equivalent field.
    pub sender: Option<String>,

    // ── Position bounds ───────────────────────────────────────────────────
    /// Lower tick boundary of the position.
    pub tick_lower: i32,
    /// Upper tick boundary of the position.
    pub tick_upper: i32,

    // ── Liquidity & token amounts ─────────────────────────────────────────
    /// Liquidity units added (Mint) or removed (Burn) (0x-hex `u128`).
    ///
    /// This is the raw V3 liquidity delta — **not** a token amount.  To derive
    /// token amounts, apply the V3 liquidity-to-amount formulas using
    /// `sqrt_price_x96` and the tick bounds.
    pub amount: String,
    /// Token0 amount transferred into (Mint) or out of (Burn) the pool
    /// (0x-hex `u128`).
    pub amount0: String,
    /// Token1 amount transferred into (Mint) or out of (Burn) the pool
    /// (0x-hex `u128`).
    pub amount1: String,
}

impl LiquidityChangeRow {
    /// Construct a [`LiquidityChangeRow`] from decoded event fields.
    ///
    /// * `kind`    – [`LiquidityChangeKind::Mint`] or [`LiquidityChangeKind::Burn`]
    /// * `pool`    – pool contract address
    /// * `block`   – block number
    /// * `ts`      – block timestamp (Unix seconds)
    /// * `tx`      – transaction hash
    /// * `log_idx` – log index within the transaction
    /// * `owner`   – position owner
    /// * `sender`  – mint initiator (`None` for burns)
    /// * `tick_lower` / `tick_upper` – position tick bounds
    /// * `amount`  – liquidity delta (u128)
    /// * `amount0` / `amount1` – token amounts (u128, always non-negative)
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: LiquidityChangeKind,
        pool: Address,
        block: u64,
        ts: u64,
        tx: B256,
        log_idx: u32,
        owner: Address,
        sender: Option<Address>,
        tick_lower: i32,
        tick_upper: i32,
        amount: u128,
        amount0: u128,
        amount1: u128,
    ) -> Self {
        Self {
            event_type: kind.as_str().to_string(),
            pool_address: format!("{pool:?}"),
            block_number: block as i64,
            block_timestamp: ts as i64,
            tx_hash: format!("{tx:?}"),
            log_index: log_idx as i32,
            owner: format!("{owner:?}"),
            sender: sender.map(|a| format!("{a:?}")),
            tick_lower,
            tick_upper,
            amount: u128_to_hex(amount),
            amount0: u128_to_hex(amount0),
            amount1: u128_to_hex(amount1),
        }
    }
}
