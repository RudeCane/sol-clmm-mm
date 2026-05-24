//! Orca Whirlpools venue — targets the `orca_whirlpools` high-level SDK v8.0.0.
//!
//! ==========================================================================
//! STATUS (precise about verified vs to-confirm):
//!  - `fetch_state` is wired against the REAL, doc-verified v8.0.0 API:
//!      * fetch_concentrated_liquidity_pool(rpc, token_a, token_b, tick_spacing,
//!        deployment) -> PoolInfo::Initialized(InitializedPool { address, data, price })
//!      * fetch_positions_for_owner(rpc, owner, deployment) -> Vec<PositionOrBundle>
//!      * tick_index_to_price(tick, dec_a, dec_b) -> f64  [arg order VERIFIED]
//!      * try_get_token_estimates_from_liquidity(liquidity, sqrt_price,
//!        tick_lo, tick_hi, round_up) -> Result<(u64 A, u64 B)>  [sig VERIFIED]
//!    Pool price arrives as a ready f64; inventory is computed from position
//!    liquidity + current sqrt_price (real balances, no fabrication).
//!  - TO CONFIRM: the exact field names on the position data struct
//!    (`.whirlpool`, `.liquidity`, `.tick_lower_index`, `.tick_upper_index`,
//!    `.fee_owed_b`) follow the Whirlpool program's Position account; check
//!    against orca_whirlpools_client's generated `Position` for 8.0.0 if the
//!    compiler disagrees.
//!  - `ensure_position` / `recenter` remain instruction-wiring TODOs: the
//!    builders exist (open_position_instructions, decrease_liquidity_instructions,
//!    close_position_instructions, increase_liquidity_instructions) but
//!    building + signing + sending the tx is left to wire against the exact
//!    builder return shapes.
//!
//! Dependency versions: crate is on Solana v3 split crates throughout, matching
//! orca_whirlpools 8.0.0. Gated behind `#[cfg(feature = "orca")]`.
//! ==========================================================================

use super::{PoolState, RebalanceReceipt, Venue};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use std::sync::Arc;

use orca_whirlpools::{
    fetch_concentrated_liquidity_pool, fetch_positions_for_owner, PoolInfo, PositionOrBundle,
};
use orca_whirlpools_core::{tick_index_to_price, try_get_token_estimates_from_liquidity};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

/// Orca needs the two token mints + tick spacing to resolve a pool (the
/// high-level fetch is by token-pair + spacing, not by pool address).
pub struct OrcaVenue {
    rpc: RpcClient,
    token_a: Pubkey,
    token_b: Pubkey,
    tick_spacing: u16,
    /// Decimals for token A and B, to convert raw amounts -> UI amounts.
    decimals_a: u8,
    decimals_b: u8,
    wallet: Arc<Keypair>,
    dry_run: bool,
}

impl OrcaVenue {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rpc_url: String,
        token_a: Pubkey,
        token_b: Pubkey,
        tick_spacing: u16,
        decimals_a: u8,
        decimals_b: u8,
        wallet: Arc<Keypair>,
        dry_run: bool,
    ) -> Self {
        Self {
            rpc: RpcClient::new(rpc_url),
            token_a,
            token_b,
            tick_spacing,
            decimals_a,
            decimals_b,
            wallet,
            dry_run,
        }
    }
}

#[async_trait]
impl Venue for OrcaVenue {
    fn name(&self) -> &'static str {
        "Orca Whirlpools"
    }

    async fn fetch_state(&self) -> Result<PoolState> {
        // --- Pool: VERIFIED v8.0.0 API ---------------------------------------
        // Default deployment (mainnet) when passing None.
        let pool_info = fetch_concentrated_liquidity_pool(
            &self.rpc,
            self.token_a,
            self.token_b,
            self.tick_spacing,
            None,
        )
        .await
        .map_err(|e| anyhow!("fetch_concentrated_liquidity_pool: {e}"))?;

        let pool = match pool_info {
            PoolInfo::Initialized(p) => p,
            PoolInfo::Uninitialized(_) => {
                bail!("Orca pool for this token pair / tick spacing is not initialized")
            }
        };

        // High-level SDK hands us price as a ready f64.
        let price = pool.price;
        // Current sqrt price (Q64.64) lives in the Whirlpool account data; needed
        // for the liquidity->token-amount estimate below.
        let current_sqrt_price: u128 = pool.data.sqrt_price;

        // --- Position fetch: VERIFIED API. Field access on the position data
        // struct (`.whirlpool`, `.liquidity`, `.tick_lower_index`,
        // `.tick_upper_index`, `.fee_owed_b`) follows the Whirlpool program's
        // Position account; confirm exact names against orca_whirlpools_client
        // generated `Position` for 8.0.0 if the compiler complains. ----------
        let positions = fetch_positions_for_owner(&self.rpc, self.wallet.pubkey(), None)
            .await
            .map_err(|e| anyhow!("fetch_positions_for_owner: {e}"))?;

        let mut range_lower = None;
        let mut range_upper = None;
        let mut inventory_a = 0.0;
        let mut inventory_b = 0.0;
        let mut fees_accrued_b = 0.0;

        for pos in positions {
            // We only handle standalone positions (not bundles) here.
            if let PositionOrBundle::Position(hydrated) = pos {
                let data = &hydrated.data;
                if data.whirlpool != pool.address {
                    continue;
                }
                let lower_tick = data.tick_lower_index;
                let upper_tick = data.tick_upper_index;

                // Price bounds: tick_index_to_price arg order VERIFIED in core docs.
                range_lower = Some(tick_index_to_price(lower_tick, self.decimals_a, self.decimals_b));
                range_upper = Some(tick_index_to_price(upper_tick, self.decimals_a, self.decimals_b));

                // Inventory: VERIFIED core fn. Signature:
                //   try_get_token_estimates_from_liquidity(
                //     liquidity_delta: u128, current_sqrt_price: u128,
                //     tick_lower_index: i32, tick_upper_index: i32, round_up: bool)
                //   -> Result<(u64 /*A*/, u64 /*B*/), &str>
                // round_up=false to estimate current holdings (not a max-deposit).
                match try_get_token_estimates_from_liquidity(
                    data.liquidity,
                    current_sqrt_price,
                    lower_tick,
                    upper_tick,
                    false,
                ) {
                    Ok((amt_a, amt_b)) => {
                        inventory_a = amt_a as f64 / 10f64.powi(self.decimals_a as i32);
                        inventory_b = amt_b as f64 / 10f64.powi(self.decimals_b as i32);
                    }
                    Err(e) => tracing::warn!("token estimate failed: {e}"),
                }

                fees_accrued_b = data.fee_owed_b as f64 / 10f64.powi(self.decimals_b as i32);
                break;
            }
        }

        Ok(PoolState {
            price,
            range_lower,
            range_upper,
            inventory_a,
            inventory_b,
            fees_accrued_b,
        })
    }

    async fn ensure_position(&self, _half_width_bps: u32) -> Result<()> {
        // Build with open_position_instructions(rpc, whirlpool, lower_price,
        // upper_price, liquidity_param, slippage, funder) then sign+send.
        // Honor dry_run: log + return Ok without sending.
        if self.dry_run {
            tracing::warn!("Orca ensure_position: dry_run, not opening");
            return Ok(());
        }
        let _ = &self.wallet;
        bail!("Orca ensure_position: instruction wiring not yet completed")
    }

    async fn recenter(&self, _half_width_bps: u32) -> Result<RebalanceReceipt> {
        if self.dry_run {
            bail!("Orca recenter called in dry_run — real tx path not yet wired");
        }
        // decrease_liquidity_instructions -> close_position_instructions ->
        // open_position_instructions -> increase_liquidity_instructions, each
        // signed with self.wallet and sent via self.rpc; collect sigs + costs.
        bail!("Orca recenter: instruction wiring not yet completed")
    }
}
