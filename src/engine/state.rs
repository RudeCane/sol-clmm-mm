//! Shared state between the rebalance engine and the web layer, plus the
//! command enum the dashboard sends in. State is held behind an Arc<RwLock>.

use crate::venues::PoolState;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Commands the dashboard can issue to the engine.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Start,
    Stop,
    Recenter,
}

/// A single line in the rolling activity log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: u64,
    pub level: String,
    pub msg: String,
}

/// The full snapshot streamed to the dashboard each tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub venue: String,
    pub running: bool,
    pub dry_run: bool,
    pub pool: Option<PoolState>,
    pub in_range: bool,
    pub half_width_bps: u32,
    pub trigger_bps: u32,
    /// Cumulative realized PnL (token B) across all recenters this session.
    pub cum_realized_pnl_b: f64,
    /// Cumulative rebalance cost (token B) this session.
    pub cum_cost_b: f64,
    pub rebalances: u64,
    pub log: Vec<LogEntry>,
}

pub struct SharedState {
    pub snapshot: RwLock<Snapshot>,
    pub log: RwLock<VecDeque<LogEntry>>,
}

impl SharedState {
    pub fn new(venue: String, dry_run: bool, half_width_bps: u32, trigger_bps: u32) -> Self {
        Self {
            snapshot: RwLock::new(Snapshot {
                venue,
                running: false,
                dry_run,
                pool: None,
                in_range: false,
                half_width_bps,
                trigger_bps,
                cum_realized_pnl_b: 0.0,
                cum_cost_b: 0.0,
                rebalances: 0,
                log: Vec::new(),
            }),
            log: RwLock::new(VecDeque::with_capacity(256)),
        }
    }

    pub async fn push_log(&self, level: &str, msg: impl Into<String>) {
        let entry = LogEntry {
            ts: now_secs(),
            level: level.to_string(),
            msg: msg.into(),
        };
        let mut log = self.log.write().await;
        if log.len() >= 200 {
            log.pop_front();
        }
        log.push_back(entry);
    }

    /// Copy the rolling log into the snapshot for streaming.
    pub async fn sync_log_into_snapshot(&self) {
        let log: Vec<LogEntry> = self.log.read().await.iter().cloned().collect();
        self.snapshot.write().await.log = log;
    }
}

pub fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Handle bundling the shared state and the command channel sender.
#[derive(Clone)]
pub struct AppHandle {
    pub state: Arc<SharedState>,
    pub cmd_tx: mpsc::Sender<Command>,
}
