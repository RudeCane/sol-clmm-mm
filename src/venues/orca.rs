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
    close_position_instructions, fetch_concentrated_liquidity_pool, fetch_positions_for_owner,
    open_position_instructions, ClosePositionConfig, IncreaseLiquidityParam, OpenPositionConfig,
    PoolInfo, PositionOrBundle,
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
    /// Max deposit amounts (raw base units) used when opening/funding a position.
    deposit_max_a: u64,
    deposit_max_b: u64,
    /// Slippage tolerance in bps for open/close quotes.
    slippage_bps: u16,
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
        deposit_max_a: u64,
        deposit_max_b: u64,
        slippage_bps: u16,
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
            deposit_max_a,
            deposit_max_b,
            slippage_bps,
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

    async fn ensure_position(&self, half_width_bps: u32) -> Result<()> {
        // If a position for this pool already exists, nothing to do.
        let state = self.fetch_state().await?;
        if state.has_position() {
            return Ok(());
        }
        let price = state.price;
        let (lower, upper) = half_width_bounds(price, half_width_bps);

        let param = IncreaseLiquidityParam {
            token_max_a: self.deposit_max_a,
            token_max_b: self.deposit_max_b,
        };
        let config = OpenPositionConfig {
            slippage_tolerance_bps: Some(self.slippage_bps),
            funder: Some(self.wallet.pubkey()),
            whirlpool_deployment: None,
        };

        let pool = self.pool_address().await?;
        let open = open_position_instructions(&self.rpc, pool, lower, upper, param, config)
            .await
            .map_err(|e| anyhow!("open_position_instructions: {e}"))?;

        if self.dry_run {
            tracing::warn!(
                "Orca ensure_position: dry_run, NOT sending {} ix (mint would be {})",
                open.instructions.len(),
                open.position_mint
            );
            return Ok(());
        }

        let sig = self
            .send(open.instructions, open.additional_signers)
            .await?;
        tracing::info!(%sig, mint = %open.position_mint, "opened position");
        Ok(())
    }

    async fn recenter(&self, half_width_bps: u32) -> Result<RebalanceReceipt> {
        // Need the current position's mint to close it. fetch its mint first.
        let (pool_addr, position_mint, value_before_b) = self.current_position().await?;

        // 1. Close: collects fees+rewards, removes liquidity, closes the account.
        let close = close_position_instructions(
            &self.rpc,
            position_mint,
            ClosePositionConfig {
                slippage_tolerance_bps: Some(self.slippage_bps),
                authority: Some(self.wallet.pubkey()),
                whirlpool_deployment: None,
            },
        )
        .await
        .map_err(|e| anyhow!("close_position_instructions: {e}"))?;

        // 2. Open recentered position around current price.
        let price = self.price_now().await?;
        let (lower, upper) = half_width_bounds(price, half_width_bps);
        let param = IncreaseLiquidityParam {
            token_max_a: self.deposit_max_a,
            token_max_b: self.deposit_max_b,
        };
        let config = OpenPositionConfig {
            slippage_tolerance_bps: Some(self.slippage_bps),
            funder: Some(self.wallet.pubkey()),
            whirlpool_deployment: None,
        };
        let open = open_position_instructions(&self.rpc, pool_addr, lower, upper, param, config)
            .await
            .map_err(|e| anyhow!("open_position_instructions: {e}"))?;

        if self.dry_run {
            bail!(
                "Orca recenter: dry_run, NOT sending (close {} ix + open {} ix, new range [{:.6}, {:.6}])",
                close.instructions.len(),
                open.instructions.len(),
                lower,
                upper
            );
        }

        // Send as two separate transactions: close, then open. Keeping them
        // separate avoids exceeding the tx size/CU limits and means a failed
        // open doesn't strand a half-closed position.
        let mut signatures = Vec::new();
        let close_sig = self
            .send(close.instructions, close.additional_signers)
            .await
            .map_err(|e| anyhow!("send close tx: {e}"))?;
        signatures.push(close_sig.clone());

        let open_sig = self
            .send(open.instructions, open.additional_signers)
            .await
            .map_err(|e| anyhow!("send open tx (position was CLOSED, funds in wallet): {e}"))?;
        signatures.push(open_sig);

        // Realized PnL: the close quote tells us what we got back; compare to the
        // value the position had when this loop last opened it. We approximate
        // realized in token-B terms using the close quote's token estimates.
        let got_b = close.quote.token_est_b as f64 / 10f64.powi(self.decimals_b as i32);
        let got_a = close.quote.token_est_a as f64 / 10f64.powi(self.decimals_a as i32);
        let value_after_b = got_b + got_a * price;
        let realized_pnl_b = value_after_b - value_before_b;

        // Cost of the rebalance: the new position's initialization cost (rent
        // for the position NFT + token accounts), in lamports -> SOL. This is
        // NOT in token-B terms; we report SOL cost here and label it as such in
        // the dashboard log. (Some rent is reclaimed by the close; net cost is
        // typically just tx fees + any non-reclaimable rent. Treat as an upper
        // bound.) If you want this in token-B terms, multiply by a SOL/token-B
        // price you fetch separately.
        let cost_b = open.initialization_cost as f64 / 1e9;

        Ok(RebalanceReceipt {
            new_lower: lower,
            new_upper: upper,
            realized_pnl_b,
            cost_b,
            signatures,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute lower/upper price bounds for a half-width in bps around `price`.
fn half_width_bounds(price: f64, half_width_bps: u32) -> (f64, f64) {
    let w = half_width_bps as f64 / 10_000.0;
    (price * (1.0 - w), price * (1.0 + w))
}

impl OrcaVenue {
    /// Resolve this venue's whirlpool address from the token pair + spacing.
    async fn pool_address(&self) -> Result<Pubkey> {
        match fetch_concentrated_liquidity_pool(
            &self.rpc,
            self.token_a,
            self.token_b,
            self.tick_spacing,
            None,
        )
        .await
        .map_err(|e| anyhow!("fetch pool: {e}"))?
        {
            PoolInfo::Initialized(p) => Ok(p.address),
            PoolInfo::Uninitialized(_) => bail!("pool not initialized"),
        }
    }

    async fn price_now(&self) -> Result<f64> {
        Ok(self.fetch_state().await?.price)
    }

    /// Find the wallet's current position in this pool: returns
    /// (pool_address, position_mint, current_value_in_b).
    async fn current_position(&self) -> Result<(Pubkey, Pubkey, f64)> {
        let pool = self.pool_address().await?;
        let state = self.fetch_state().await?;
        let value_b = state.inventory_b + state.inventory_a * state.price;

        let positions = fetch_positions_for_owner(&self.rpc, self.wallet.pubkey(), None)
            .await
            .map_err(|e| anyhow!("fetch_positions_for_owner: {e}"))?;
        for pos in positions {
            if let PositionOrBundle::Position(hydrated) = pos {
                if hydrated.data.whirlpool == pool {
                    // EXTRAPOLATED field: the NFT mint of the position. On the
                    // generated Position account this is `position_mint`.
                    return Ok((pool, hydrated.data.position_mint, value_b));
                }
            }
        }
        bail!("no open position to recenter")
    }

    /// Build, sign, and send+confirm a v3 transaction from instructions plus
    /// any extra signers the builder returned (e.g. the new position mint).
    async fn send(
        &self,
        instructions: Vec<solana_instruction::Instruction>,
        additional_signers: Vec<Keypair>,
    ) -> Result<String> {
        use solana_message::Message;
        use solana_transaction::Transaction;

        let blockhash = self
            .rpc
            .get_latest_blockhash()
            .await
            .map_err(|e| anyhow!("get_latest_blockhash: {e}"))?;

        let payer = self.wallet.pubkey();
        let msg = Message::new(&instructions, Some(&payer));

        // Signers: wallet first (payer), then any additional signers.
        let mut signers: Vec<&Keypair> = vec![self.wallet.as_ref()];
        for kp in &additional_signers {
            signers.push(kp);
        }

        let mut tx = Transaction::new_unsigned(msg);
        tx.try_sign(&signers, blockhash)
            .map_err(|e| anyhow!("sign tx: {e}"))?;

        let sig = self
            .rpc
            .send_and_confirm_transaction(&tx)
            .await
            .map_err(|e| anyhow!("send_and_confirm: {e}"))?;
        Ok(sig.to_string())
    }
}
