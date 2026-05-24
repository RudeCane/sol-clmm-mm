//! The `Venue` trait is the single abstraction both Orca Whirlpools and
//! Raydium CLMM implement. The engine only ever talks to a `dyn Venue`, so
//! swapping venues (or the simulator) never touches engine logic.
//!
//! All prices are expressed as `f64` for dashboard simplicity. Internally a
//! real venue impl should keep sqrt-price / tick math in integer space and
//! only convert at this boundary.

#[cfg(feature = "orca")]
pub mod orca;
#[cfg(feature = "raydium")]
pub mod raydium;
pub mod simulated;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::config::{Config, VenueKind};

/// Build the configured venue. The engine holds the result as `Arc<dyn Venue>`.
pub fn build(cfg: &Config) -> Result<Arc<dyn Venue>> {
    match cfg.venue {
        VenueKind::Simulated => {
            Ok(Arc::new(simulated::SimulatedVenue::new(cfg.sim_start_price)))
        }
        VenueKind::Orca => {
            #[cfg(feature = "orca")]
            {
                use std::str::FromStr;
                let wallet = cfg.load_wallet()?;
                let token_a = solana_pubkey::Pubkey::from_str(&cfg.orca_token_a)
                    .map_err(|e| anyhow::anyhow!("orca_token_a: {e}"))?;
                let token_b = solana_pubkey::Pubkey::from_str(&cfg.orca_token_b)
                    .map_err(|e| anyhow::anyhow!("orca_token_b: {e}"))?;
                Ok(Arc::new(orca::OrcaVenue::new(
                    cfg.rpc_url.clone(),
                    token_a,
                    token_b,
                    cfg.orca_tick_spacing,
                    cfg.orca_decimals_a,
                    cfg.orca_decimals_b,
                    cfg.orca_deposit_max_a,
                    cfg.orca_deposit_max_b,
                    cfg.orca_slippage_bps,
                    wallet,
                    cfg.dry_run,
                )))
            }
            #[cfg(not(feature = "orca"))]
            {
                anyhow::bail!(
                    "venue = \"orca\" requires building with --features orca \
                     (pulls in orca_whirlpools 8.0.0 + Solana v3 crates)"
                )
            }
        }
        VenueKind::Raydium => {
            #[cfg(feature = "raydium")]
            {
                let (pool, wallet) = cfg.load_pool_and_wallet()?;
                Ok(Arc::new(raydium::RaydiumVenue::new(
                    cfg.rpc_url.clone(),
                    pool,
                    wallet,
                    cfg.dry_run,
                )))
            }
            #[cfg(not(feature = "raydium"))]
            {
                anyhow::bail!("venue = \"raydium\" requires building with --features raydium")
            }
        }
    }
}

/// A snapshot of pool + position state, read once per loop tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolState {
    /// Current pool price of token A in terms of token B.
    pub price: f64,
    /// Lower price bound of our active position (None if no open position).
    pub range_lower: Option<f64>,
    /// Upper price bound of our active position.
    pub range_upper: Option<f64>,
    /// Token A held inside the position.
    pub inventory_a: f64,
    /// Token B held inside the position.
    pub inventory_b: f64,
    /// Fees accrued (in token B terms) since the position was opened.
    pub fees_accrued_b: f64,
}

impl PoolState {
    /// Is the current price inside our position's range?
    pub fn in_range(&self) -> bool {
        match (self.range_lower, self.range_upper) {
            (Some(lo), Some(hi)) => self.price >= lo && self.price <= hi,
            _ => false,
        }
    }

    pub fn has_position(&self) -> bool {
        self.range_lower.is_some() && self.range_upper.is_some()
    }
}

/// Result of a rebalance (close + reopen) action.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RebalanceReceipt {
    pub new_lower: f64,
    pub new_upper: f64,
    /// Realized PnL from closing the old position (token B terms). Captures
    /// the impermanent-loss realization the README warns about.
    pub realized_pnl_b: f64,
    /// Transaction + swap fees paid to perform the rebalance (token B terms).
    pub cost_b: f64,
    /// Tx signature(s) for audit, if any.
    pub signatures: Vec<String>,
}

#[async_trait]
pub trait Venue: Send + Sync {
    /// Human-readable venue name for the dashboard.
    fn name(&self) -> &'static str;

    /// Read current pool + position state.
    async fn fetch_state(&self) -> Result<PoolState>;

    /// Close the current position (if any) and open a new one centered on the
    /// current price with the configured half-width. Returns what it cost.
    ///
    /// `half_width_bps` is the half-width of the range in basis points around
    /// the current price (e.g. 250 = +/-2.5%).
    async fn recenter(&self, half_width_bps: u32) -> Result<RebalanceReceipt>;

    /// Open an initial position if none exists. Idempotent: a no-op if one is open.
    async fn ensure_position(&self, half_width_bps: u32) -> Result<()>;
}
