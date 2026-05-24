//! Configuration: loaded from `config.toml` plus a wallet keypair file path.
//! The wallet key is NEVER read from the TOML and NEVER logged.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VenueKind {
    Simulated,
    Orca,
    Raydium,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
// Several fields (pool, orca_*, wallet_path) are only read under venue feature
// flags; serde always populates them, so silence dead_code in the default build.
#[allow(dead_code)]
pub struct Config {
    /// Which venue to run.
    pub venue: VenueKind,

    /// Solana RPC endpoint (ignored by the simulator).
    #[serde(default = "default_rpc")]
    pub rpc_url: String,

    /// CLMM pool address (ignored by the simulator).
    #[serde(default)]
    pub pool: String,

    // ---- Orca-specific: the high-level SDK resolves a pool by token pair +
    // tick spacing, not a pool address. ----
    /// Token A mint (base).
    #[serde(default)]
    pub orca_token_a: String,
    /// Token B mint (quote).
    #[serde(default)]
    pub orca_token_b: String,
    /// Pool tick spacing (e.g. 64).
    #[serde(default)]
    pub orca_tick_spacing: u16,
    /// Decimals of token A.
    #[serde(default)]
    pub orca_decimals_a: u8,
    /// Decimals of token B.
    #[serde(default)]
    pub orca_decimals_b: u8,
    /// Max deposit of token A (raw base units) when opening/funding a position.
    #[serde(default)]
    pub orca_deposit_max_a: u64,
    /// Max deposit of token B (raw base units).
    #[serde(default)]
    pub orca_deposit_max_b: u64,
    /// Slippage tolerance (bps) for open/close quotes.
    #[serde(default = "default_slippage")]
    pub orca_slippage_bps: u16,
    /// Orca network config: "mainnet" | "devnet" | "eclipse_mainnet" |
    /// "eclipse_testnet". Selects which WhirlpoolsConfig the SDK resolves pools
    /// against. MUST match your rpc_url's cluster or pool lookups will fail.
    #[serde(default = "default_orca_network")]
    pub orca_network: String,

    /// Path to the wallet keypair JSON (Solana CLI format). Never committed.
    #[serde(default)]
    pub wallet_path: String,

    /// Half-width of the LP range in basis points (e.g. 250 = +/-2.5%).
    #[serde(default = "default_half_width")]
    pub half_width_bps: u32,

    /// How far (bps) price may drift past a range edge before we recenter.
    /// A buffer beyond the edge prevents thrashing at the boundary.
    #[serde(default = "default_trigger")]
    pub rebalance_trigger_bps: u32,

    /// Loop interval in seconds.
    #[serde(default = "default_interval")]
    pub loop_interval_secs: u64,

    /// Minimum seconds between recenters, regardless of price (rate limit /
    /// anti-thrash). Protects against fee bleed.
    #[serde(default = "default_cooldown")]
    pub rebalance_cooldown_secs: u64,

    /// If true, never send transactions — log intended actions only.
    /// Defaults to TRUE so a first run can't touch mainnet funds by accident.
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,

    /// Starting price for the simulator only.
    #[serde(default = "default_sim_price")]
    pub sim_start_price: f64,

    /// Local dashboard bind port (always 127.0.0.1).
    #[serde(default = "default_port")]
    pub web_port: u16,
}

fn default_rpc() -> String { "https://api.mainnet-beta.solana.com".into() }
fn default_half_width() -> u32 { 250 }
fn default_trigger() -> u32 { 50 }
fn default_interval() -> u64 { 10 }
fn default_cooldown() -> u64 { 300 }
fn default_dry_run() -> bool { true }
fn default_sim_price() -> f64 { 100.0 }
fn default_port() -> u16 { 8787 }
fn default_slippage() -> u16 { 100 }
fn default_orca_network() -> String { "mainnet".into() }

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {path}"))?;
        let cfg: Config = toml::from_str(&text).context("parsing config.toml")?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        if self.half_width_bps == 0 {
            anyhow::bail!("half_width_bps must be > 0");
        }
        if self.venue != VenueKind::Simulated {
            if self.pool.is_empty() {
                anyhow::bail!("pool address required for venue {:?}", self.venue);
            }
            if self.wallet_path.is_empty() {
                anyhow::bail!("wallet_path required for venue {:?}", self.venue);
            }
        }
        Ok(())
    }

    /// Load just the wallet keypair. Used by venues that resolve the pool by
    /// other means (e.g. Orca by token pair). Returns the Solana v3
    /// `solana_keypair::Keypair`.
    #[allow(dead_code)]
    pub fn load_wallet(&self) -> Result<Arc<Keypair>> {
        let bytes = std::fs::read_to_string(&self.wallet_path)
            .with_context(|| format!("reading wallet {}", self.wallet_path))?;
        let key_bytes: Vec<u8> =
            serde_json::from_str(&bytes).context("wallet file must be a JSON byte array")?;
        // v3 split crates removed Keypair::from_bytes; use try_from(&[u8]).
        let wallet = Keypair::try_from(&key_bytes[..])
            .map_err(|e| anyhow::anyhow!("invalid keypair bytes: {e}"))?;
        tracing::info!(pubkey = %wallet.pubkey(), "wallet loaded");
        Ok(Arc::new(wallet))
    }

    /// Load the pool pubkey and wallet keypair. Used by real venues only.
    #[allow(dead_code)]
    pub fn load_pool_and_wallet(&self) -> Result<(Pubkey, Arc<Keypair>)> {
        let pool = Pubkey::from_str(&self.pool)
            .with_context(|| format!("invalid pool pubkey: {}", self.pool))?;
        let wallet = self.load_wallet()?;
        Ok((pool, wallet))
    }
}
