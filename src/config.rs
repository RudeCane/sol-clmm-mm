//! Configuration: loaded from `config.toml` plus a wallet keypair file path.
//! The wallet key is NEVER read from the TOML and NEVER logged.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::{Keypair, Signer};
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
pub struct Config {
    /// Which venue to run.
    pub venue: VenueKind,

    /// Solana RPC endpoint (ignored by the simulator).
    #[serde(default = "default_rpc")]
    pub rpc_url: String,

    /// CLMM pool address (ignored by the simulator).
    #[serde(default)]
    pub pool: String,

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

    /// Load the pool pubkey and wallet keypair. Used by real venues only.
    pub fn load_pool_and_wallet(&self) -> Result<(Pubkey, Arc<Keypair>)> {
        let pool = Pubkey::from_str(&self.pool)
            .with_context(|| format!("invalid pool pubkey: {}", self.pool))?;
        let bytes = std::fs::read_to_string(&self.wallet_path)
            .with_context(|| format!("reading wallet {}", self.wallet_path))?;
        let key_bytes: Vec<u8> =
            serde_json::from_str(&bytes).context("wallet file must be a JSON byte array")?;
        let wallet = Keypair::from_bytes(&key_bytes).context("invalid keypair bytes")?;
        tracing::info!(pubkey = %wallet.pubkey(), "wallet loaded");
        Ok((pool, Arc::new(wallet)))
    }
}
