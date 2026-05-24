//! Raydium CLMM venue.
//!
//! ==========================================================================
//! THIS FILE IS A WIRING TEMPLATE, NOT COMPILE-TESTED AGAINST THE REAL SDK.
//! ==========================================================================
//! Same contract as orca.rs. Raydium's CLMM program + account layout differ
//! from Orca's Whirlpools (different pool struct, tick array layout, and
//! instruction set), so do not copy Orca calls here. Verify every call against
//! the Raydium CLMM SDK / program IDL for the version you depend on.
//!
//! Rough mapping:
//!   fetch_state    -> fetch PoolState account + your personal position;
//!                     convert sqrt_price_x64 -> price, tick bounds -> prices.
//!   ensure_position-> open_position if none held.
//!   recenter       -> decrease_liquidity(all) + close_position, then
//!                     open_position + increase_liquidity at new ticks.

use super::{PoolState, RebalanceReceipt, Venue};
use anyhow::{bail, Result};
use async_trait::async_trait;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use std::sync::Arc;

pub struct RaydiumVenue {
    pub rpc_url: String,
    pub pool: Pubkey,
    pub wallet: Arc<Keypair>,
    pub dry_run: bool,
}

impl RaydiumVenue {
    pub fn new(
        rpc_url: String,
        pool: Pubkey,
        wallet: Arc<Keypair>,
        dry_run: bool,
    ) -> Self {
        Self { rpc_url, pool, wallet, dry_run }
    }
}

#[async_trait]
impl Venue for RaydiumVenue {
    fn name(&self) -> &'static str {
        "Raydium CLMM"
    }

    async fn fetch_state(&self) -> Result<PoolState> {
        todo!("Raydium: fetch pool + position accounts, convert sqrt_price_x64/ticks -> PoolState")
    }

    async fn ensure_position(&self, _half_width_bps: u32) -> Result<()> {
        todo!("Raydium: open initial position if none, honoring dry_run")
    }

    async fn recenter(&self, _half_width_bps: u32) -> Result<RebalanceReceipt> {
        if self.dry_run {
            bail!("Raydium recenter called in dry_run — real path not yet wired; \
                   run the simulator or wire the SDK calls first");
        }
        todo!("Raydium: close old position + open recentered position")
    }
}
