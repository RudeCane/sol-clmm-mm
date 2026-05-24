//! Orca Whirlpools venue — targets the `orca_whirlpools` high-level SDK v8.0.0.
//!
//! ==========================================================================
//! STATUS (be precise about what's verified vs extrapolated):
//!  - `fetch_state` is wired against the REAL, doc-verified v8.0.0 API:
//!      * fetch_concentrated_liquidity_pool(rpc, token_a, token_b, tick_spacing,
//!        deployment) -> PoolInfo::Initialized(InitializedPool { address, data, price })
//!      * fetch_positions_for_owner(rpc, owner, deployment) -> Vec<PositionOrBundle>
//!    The pool price arrives as a ready f64 (no manual sqrt-price math needed).
//!  - The position->price-bound conversion and the exact PositionOrBundle field
//!    names are marked EXTRAPOLATED below: verify against
//!    https://docs.rs/orca_whirlpools/8.0.0 and orca_whirlpools_core
//!    (price_to_tick_index / tick_index_to_price) before mainnet use.
//!  - `ensure_position` / `recenter` remain instruction-wiring TODOs: the
//!    open/close/increase/decrease instruction builders exist
//!    (open_position_instructions, decrease_liquidity_instructions,
//!    close_position_instructions, increase_liquidity_instructions) but building
//!    + signing + sending the tx is left to you so it can be checked against the
//!    exact builder return shapes.
//!
//! IMPORTANT — dependency versions: orca_whirlpools 8.0.0 pulls in Solana v3
//! crates (solana-client ^3, solana-keypair ^3, ...). The workspace Cargo.toml
//! currently pins solana 2.0 for the simulator-only build. Building the real
//! Orca path requires bumping those to 3.x; see the `orca` feature note in
//! Cargo.toml. This file is gated behind `#[cfg(feature = "orca")]`.
//! ==========================================================================

use super::{PoolState, RebalanceReceipt, Venue};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use std::sync::Arc;

use orca_whirlpools::{
    fetch_concentrated_liquidity_pool, fetch_positions_for_owner, PoolInfo, PositionOrBundle,
};
use orca_whirlpools_core::{sqrt_price_to_price, tick_index_to_price};
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

        // --- Position: VERIFIED fetch, EXTRAPOLATED field access -------------
        // fetch_positions_for_owner returns every position the wallet owns
        // across all pools; we filter to this pool by whirlpool address.
        let positions = fetch_positions_for_owner(&self.rpc, self.wallet.pubkey(), None)
            .await
            .map_err(|e| anyhow!("fetch_positions_for_owner: {e}"))?;

        let mut range_lower = None;
        let mut range_upper = None;
        let mut inventory_a = 0.0;
        let mut inventory_b = 0.0;
        let mut fees_accrued_b = 0.0;

        for pos in positions {
            // EXTRAPOLATED: PositionOrBundle is an enum of a standalone Position
            // vs a PositionBundle. We only handle standalone here. Verify the
            // variant name + inner field names (`.position`, `.data`, etc.)
            // against the v8.0.0 docs — they are NOT confirmed in this draft.
            if let PositionOrBundle::Position(hydrated) = pos {
                let data = &hydrated.data; // Position account data
                if data.whirlpool != pool.address {
                    continue;
                }
                // tick_index_to_price(tick, decimals_a, decimals_b) -> f64
                // (function exists in orca_whirlpools_core; arg order verified
                // by example in core docs).
                range_lower = Some(tick_index_to_price(
                    data.tick_lower_index,
                    self.decimals_a,
                    self.decimals_b,
                ));
                range_upper = Some(tick_index_to_price(
                    data.tick_upper_index,
                    self.decimals_a,
                    self.decimals_b,
                ));
                // EXTRAPOLATED: liquidity -> token amounts needs
                // orca_whirlpools_core::get_token_amounts_from_liquidity(...)
                // using current sqrt_price + tick bounds. Left as a follow-up;
                // for now we surface fees and leave inventory at 0 until that
                // conversion is wired, so the dashboard is honest rather than
                // showing fabricated balances.
                let _ = (&mut inventory_a, &mut inventory_b);
                fees_accrued_b = (data.fee_owed_b as f64) / 10f64.powi(self.decimals_b as i32);
                break;
            }
        }

        let _ = sqrt_price_to_price; // referenced to document availability

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
