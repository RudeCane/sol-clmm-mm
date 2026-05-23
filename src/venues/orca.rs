//! Orca Whirlpools venue.
//!
//! ==========================================================================
//! THIS FILE IS A WIRING TEMPLATE, NOT COMPILE-TESTED AGAINST THE REAL SDK.
//! ==========================================================================
//! Every `todo!()` below marks a spot where a real Orca Whirlpools SDK 7.x
//! call goes. The function SIGNATURES and the surrounding engine contract are
//! correct and final — you only fill the bodies. Check each call against the
//! actual crate docs for the exact version in your Cargo.lock; the API has
//! shifted across 7.x point releases, so do not trust any remembered call here.
//!
//! Suggested crate (verify version/name yourself):
//!   orca_whirlpools = "..."   # the high-level client
//!   orca_whirlpools_core      # tick / sqrt-price math
//!
//! Rough mapping of what each method needs to do:
//!   fetch_state    -> fetch Whirlpool account + your position account, convert
//!                     sqrt_price -> price, read tick_lower/upper -> price bounds,
//!                     read token amounts + accrued fees.
//!   ensure_position-> if no position NFT held for this pool, open one.
//!   recenter       -> decrease_liquidity to 0 + close_position on the old one,
//!                     then open_position + increase_liquidity centered on price.

use super::{PoolState, RebalanceReceipt, Venue};
use anyhow::{bail, Result};
use async_trait::async_trait;
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;

pub struct OrcaVenue {
    pub rpc_url: String,
    pub pool: Pubkey,
    pub wallet: Arc<solana_sdk::signature::Keypair>,
    pub dry_run: bool,
}

impl OrcaVenue {
    pub fn new(
        rpc_url: String,
        pool: Pubkey,
        wallet: Arc<solana_sdk::signature::Keypair>,
        dry_run: bool,
    ) -> Self {
        Self { rpc_url, pool, wallet, dry_run }
    }
}

#[async_trait]
impl Venue for OrcaVenue {
    fn name(&self) -> &'static str {
        "Orca Whirlpools"
    }

    async fn fetch_state(&self) -> Result<PoolState> {
        // 1. Fetch the Whirlpool account for self.pool.
        // 2. Convert sqrt_price (Q64.64) -> human price.
        // 3. Fetch your position account(s) for this pool; if present, convert
        //    tick_lower_index / tick_upper_index -> price bounds, and read the
        //    token amounts + fee_owed_a/b.
        // 4. Populate PoolState (fees in token B terms).
        todo!("Orca: fetch whirlpool + position, convert sqrt_price/ticks -> PoolState")
    }

    async fn ensure_position(&self, _half_width_bps: u32) -> Result<()> {
        // If no position NFT is held for this pool, open_position +
        // increase_liquidity centered on current price with the half-width.
        // Respect self.dry_run: when true, log the intended ix and return Ok
        // WITHOUT sending.
        todo!("Orca: open initial position if none, honoring dry_run")
    }

    async fn recenter(&self, _half_width_bps: u32) -> Result<RebalanceReceipt> {
        if self.dry_run {
            // In dry-run we must not send tx. Return a zero-cost receipt so the
            // engine's loop and logging still exercise the full path.
            bail!("Orca recenter called in dry_run — real path not yet wired; \
                   run the simulator or wire the SDK calls first");
        }
        // 1. decrease_liquidity to zero on the current position.
        // 2. collect_fees + collect_reward if any.
        // 3. close_position (reclaims rent).
        // 4. open_position with new tick range centered on price.
        // 5. increase_liquidity with the freed tokens.
        // 6. Build RebalanceReceipt with realized pnl, cost, and tx sigs.
        todo!("Orca: close old position + open recentered position")
    }
}
