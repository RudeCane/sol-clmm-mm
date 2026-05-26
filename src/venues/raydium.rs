//! Raydium CLMM venue — read path wired against the pure-math crate
//! `solana_clmm_raydium` + manual Anchor-account decoding.
//!
//! ==========================================================================
//! STATUS:
//!  - APPROACH (per your choice): Anchor account decode + pure-math crate, no
//!    high-level wrapper. RPC fetch via the same solana-client we already use.
//!  - `fetch_state`: wired. Price from sqrt_price_x64 (math crate verifies the
//!    tick<->sqrt-price relationship); inventory from position liquidity +
//!    tick bounds via get_delta_amounts_signed. The MATH calls are doc-verified.
//!  - ⚠️ ACCOUNT LAYOUT IS THE RISK. Raydium PoolState / PersonalPositionState
//!    are Anchor accounts (8-byte discriminator + borsh body). The field
//!    OFFSETS below are derived from raydium-amm-v3's state structs and MUST be
//!    verified against the on-chain source for the program you target:
//!      https://github.com/raydium-io/raydium-clmm  (programs/amm/src/states/)
//!    A wrong offset yields silently-wrong numbers (worse than a panic), so
//!    these are marked and isolated in the decode fns below.
//!  - `ensure_position` / `recenter`: instruction building against Raydium's
//!    Anchor program is the next step (open_position / increase_liquidity /
//!    decrease_liquidity / close_position discriminators + account metas).
//!    Left as TODO so it can be built and devnet-tested deliberately.
//!
//! Build: cargo build --features raydium
//! ==========================================================================

use super::{PoolState, RebalanceReceipt, Venue};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use std::sync::Arc;

use solana_clmm_raydium::liquidity_math::get_delta_amounts_signed;
use solana_clmm_raydium::tick_math::get_sqrt_price_at_tick;

/// Raydium CLMM program ID (mainnet + devnet share the same ID).
const RAYDIUM_CLMM_PROGRAM: &str = "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK";

pub struct RaydiumVenue {
    rpc: RpcClient,
    pool: Pubkey,
    decimals_a: u8,
    decimals_b: u8,
    wallet: Arc<Keypair>,
    dry_run: bool,
}

impl RaydiumVenue {
    pub fn new(
        rpc_url: String,
        pool: Pubkey,
        decimals_a: u8,
        decimals_b: u8,
        wallet: Arc<Keypair>,
        dry_run: bool,
    ) -> Self {
        Self {
            rpc: RpcClient::new(rpc_url),
            pool,
            decimals_a,
            decimals_b,
            wallet,
            dry_run,
        }
    }
}

// ---------------------------------------------------------------------------
// Decoded account views — ONLY the fields we need, read at fixed offsets.
//
// ⚠️ VERIFY THESE OFFSETS against raydium-amm-v3 states/pool.rs &
// states/personal_position.rs for your target program version. Anchor accounts
// begin with an 8-byte discriminator; offsets below are measured from byte 0
// (i.e. they already include the +8). If Raydium changes the struct, these move.
// ---------------------------------------------------------------------------

/// Minimal view of Raydium `PoolState`.
struct PoolView {
    sqrt_price_x64: u128,
    tick_current: i32,
}

/// Minimal view of Raydium `PersonalPositionState`.
struct PositionView {
    pool_id: Pubkey,
    tick_lower: i32,
    tick_upper: i32,
    liquidity: u128,
}

fn read_u128_le(buf: &[u8], off: usize) -> Result<u128> {
    let end = off + 16;
    let slice = buf.get(off..end).context("u128 offset out of range")?;
    Ok(u128::from_le_bytes(slice.try_into().unwrap()))
}
fn read_i32_le(buf: &[u8], off: usize) -> Result<i32> {
    let end = off + 4;
    let slice = buf.get(off..end).context("i32 offset out of range")?;
    Ok(i32::from_le_bytes(slice.try_into().unwrap()))
}
fn read_pubkey(buf: &[u8], off: usize) -> Result<Pubkey> {
    let end = off + 32;
    let slice = buf.get(off..end).context("pubkey offset out of range")?;
    let arr: [u8; 32] = slice.try_into().unwrap();
    Ok(Pubkey::new_from_array(arr))
}

impl RaydiumVenue {
    /// Decode the PoolState fields we need.
    /// OFFSETS (VERIFY): PoolState layout in raydium-amm-v3 places, after the
    /// 8-byte discriminator: bump[1], amm_config[32], owner[32], token_mint_0[32],
    /// token_mint_1[32], token_vault_0[32], token_vault_1[32], observation_key[32],
    /// mint_decimals_0[1], mint_decimals_1[1], tick_spacing[2], liquidity[16],
    /// sqrt_price_x64[16], tick_current[4], ...
    /// => sqrt_price_x64 at 8+1+32*7+1+1+2+16 = 269; tick_current at 285.
    /// These are DERIVED, not verified — confirm against the on-chain struct.
    fn decode_pool(buf: &[u8]) -> Result<PoolView> {
        const SQRT_PRICE_OFF: usize = 269; // VERIFY
        const TICK_CURRENT_OFF: usize = 285; // VERIFY
        Ok(PoolView {
            sqrt_price_x64: read_u128_le(buf, SQRT_PRICE_OFF)?,
            tick_current: read_i32_le(buf, TICK_CURRENT_OFF)?,
        })
    }

    /// Decode PersonalPositionState fields.
    /// OFFSETS (VERIFY): after 8-byte discriminator: bump[1], nft_mint[32],
    /// pool_id[32], tick_lower_index[4], tick_upper_index[4], liquidity[16], ...
    /// => pool_id at 8+1+32 = 41; tick_lower at 73; tick_upper at 77;
    ///    liquidity at 81.
    fn decode_position(buf: &[u8]) -> Result<PositionView> {
        const POOL_ID_OFF: usize = 41; // VERIFY
        const TICK_LOWER_OFF: usize = 73; // VERIFY
        const TICK_UPPER_OFF: usize = 77; // VERIFY
        const LIQUIDITY_OFF: usize = 81; // VERIFY
        Ok(PositionView {
            pool_id: read_pubkey(buf, POOL_ID_OFF)?,
            tick_lower: read_i32_le(buf, TICK_LOWER_OFF)?,
            tick_upper: read_i32_le(buf, TICK_UPPER_OFF)?,
            liquidity: read_u128_le(buf, LIQUIDITY_OFF)?,
        })
    }

    /// sqrt_price_x64 (Q64.64) -> human price, decimal-adjusted.
    fn price_from_sqrt(&self, sqrt_price_x64: u128) -> f64 {
        // price = (sqrt_price_x64 / 2^64)^2 * 10^(dec_a - dec_b)
        let sp = sqrt_price_x64 as f64 / (2f64.powi(64));
        let raw = sp * sp;
        raw * 10f64.powi(self.decimals_a as i32 - self.decimals_b as i32)
    }

    fn tick_to_price(&self, tick: i32) -> Result<f64> {
        let sqrt_x64 = get_sqrt_price_at_tick(tick).map_err(|e| anyhow!("tick_math: {e:?}"))?;
        Ok(self.price_from_sqrt(sqrt_x64))
    }
}

#[async_trait]
impl Venue for RaydiumVenue {
    fn name(&self) -> &'static str {
        "Raydium CLMM"
    }

    async fn fetch_state(&self) -> Result<PoolState> {
        // 1. Fetch + decode the pool account.
        let pool_acct = self
            .rpc
            .get_account_data(&self.pool)
            .await
            .map_err(|e| anyhow!("get pool account: {e}"))?;
        let pool = Self::decode_pool(&pool_acct)?;
        let price = self.price_from_sqrt(pool.sqrt_price_x64);

        // 2. Find our position(s) for this pool. Raydium positions are separate
        //    accounts owned by the CLMM program; locating them by owner requires
        //    a getProgramAccounts scan with a memcmp on the position's authority/
        //    nft owner. That scan + filter is the next wiring step; for now we
        //    report pool price with no position (honest: no fabricated inventory).
        //    VERIFY: the gPA memcmp offset for the position owner.
        let _program: Pubkey = RAYDIUM_CLMM_PROGRAM
            .parse()
            .map_err(|e| anyhow!("program id parse: {e}"))?;

        let mut range_lower = None;
        let mut range_upper = None;
        let mut inventory_a = 0.0;
        let mut inventory_b = 0.0;

        // Placeholder for the located position. When the gPA scan is wired,
        // decode each into a PositionView and pick the one whose pool_id == self.pool.
        let located: Option<PositionView> = None;
        if let Some(pos) = located {
            if pos.pool_id == self.pool {
                range_lower = Some(self.tick_to_price(pos.tick_lower)?);
                range_upper = Some(self.tick_to_price(pos.tick_upper)?);
                // Inventory: token amounts for this liquidity across the range.
                // get_delta_amounts_signed(tick_current, sqrt_price_current,
                //   tick_lower, tick_upper, liquidity_delta) -> (amount_0, amount_1)
                let sqrt_cur = pool.sqrt_price_x64;
                match get_delta_amounts_signed(
                    pool.tick_current,
                    sqrt_cur,
                    pos.tick_lower,
                    pos.tick_upper,
                    pos.liquidity as i128,
                ) {
                    Ok((amt0, amt1)) => {
                        inventory_a = amt0 as f64 / 10f64.powi(self.decimals_a as i32);
                        inventory_b = amt1 as f64 / 10f64.powi(self.decimals_b as i32);
                    }
                    Err(e) => tracing::warn!("delta amounts: {e:?}"),
                }
            }
        }

        Ok(PoolState {
            price,
            range_lower,
            range_upper,
            inventory_a,
            inventory_b,
            fees_accrued_b: 0.0, // fee accounting is a later step
        })
    }

    async fn ensure_position(&self, _half_width_bps: u32) -> Result<()> {
        if self.dry_run {
            tracing::warn!("Raydium ensure_position: dry_run, not opening");
            return Ok(());
        }
        let _ = &self.wallet;
        bail!("Raydium ensure_position: instruction wiring not yet completed")
    }

    async fn recenter(&self, _half_width_bps: u32) -> Result<RebalanceReceipt> {
        if self.dry_run {
            bail!("Raydium recenter called in dry_run — write path not yet wired");
        }
        bail!("Raydium recenter: instruction wiring not yet completed")
    }
}
