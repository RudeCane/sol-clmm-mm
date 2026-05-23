//! The rebalance engine. Owns the venue and runs the control loop:
//! fetch state -> update snapshot -> decide whether to recenter -> act.
//!
//! Recenter decision: we recenter when price has drifted *past* a range edge
//! by more than `rebalance_trigger_bps`, AND the cooldown has elapsed. The
//! buffer + cooldown together are the anti-thrash protection against fee bleed.

pub mod state;

use crate::config::Config;
use crate::venues::Venue;
use state::{Command, SharedState};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};

pub struct Engine {
    venue: Arc<dyn Venue>,
    state: Arc<SharedState>,
    cfg: Config,
    cmd_rx: mpsc::Receiver<Command>,
    last_rebalance: u64,
    running: bool,
}

impl Engine {
    pub fn new(
        venue: Arc<dyn Venue>,
        state: Arc<SharedState>,
        cfg: Config,
        cmd_rx: mpsc::Receiver<Command>,
    ) -> Self {
        Self {
            venue,
            state,
            cfg,
            cmd_rx,
            last_rebalance: 0,
            running: false,
        }
    }

    pub async fn run(mut self) {
        let mut tick = interval(Duration::from_secs(self.cfg.loop_interval_secs.max(1)));
        self.state
            .push_log("info", format!("engine ready on venue '{}'", self.venue.name()))
            .await;
        if self.cfg.dry_run {
            self.state
                .push_log("warn", "DRY-RUN active: no transactions will be sent")
                .await;
        }

        loop {
            tokio::select! {
                // Drain any pending commands first.
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                _ = tick.tick() => {
                    if self.running {
                        self.step().await;
                    }
                    // Always refresh the snapshot's log even when stopped.
                    self.state.sync_log_into_snapshot().await;
                }
            }
        }
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Start => {
                if !self.running {
                    self.running = true;
                    self.state.snapshot.write().await.running = true;
                    self.state.push_log("info", "started").await;
                    // Make sure a position exists when we start.
                    if let Err(e) = self.venue.ensure_position(self.cfg.half_width_bps).await {
                        self.state
                            .push_log("error", format!("ensure_position failed: {e}"))
                            .await;
                    }
                }
            }
            Command::Stop => {
                if self.running {
                    self.running = false;
                    self.state.snapshot.write().await.running = false;
                    self.state.push_log("info", "stopped").await;
                }
            }
            Command::Recenter => {
                self.state.push_log("info", "manual recenter requested").await;
                self.do_recenter().await;
            }
        }
        self.state.sync_log_into_snapshot().await;
    }

    async fn step(&mut self) {
        let pool = match self.venue.fetch_state().await {
            Ok(p) => p,
            Err(e) => {
                self.state
                    .push_log("error", format!("fetch_state failed: {e}"))
                    .await;
                return;
            }
        };

        let in_range = pool.in_range();
        {
            let mut snap = self.state.snapshot.write().await;
            snap.in_range = in_range;
            snap.pool = Some(pool.clone());
        }

        if self.should_recenter(&pool) {
            self.do_recenter().await;
        }
    }

    /// Recenter only when price is past an edge by more than the trigger
    /// buffer, and the cooldown has elapsed.
    fn should_recenter(&self, pool: &crate::venues::PoolState) -> bool {
        if !pool.has_position() {
            return true; // no position -> open one via recenter
        }
        let buffer = self.cfg.rebalance_trigger_bps as f64 / 10_000.0;
        let (lo, hi) = match (pool.range_lower, pool.range_upper) {
            (Some(l), Some(h)) => (l, h),
            _ => return true,
        };
        let past_low = pool.price < lo * (1.0 - buffer);
        let past_high = pool.price > hi * (1.0 + buffer);
        let drifted = past_low || past_high;

        let cooled = state::now_secs().saturating_sub(self.last_rebalance)
            >= self.cfg.rebalance_cooldown_secs;

        drifted && cooled
    }

    async fn do_recenter(&mut self) {
        match self.venue.recenter(self.cfg.half_width_bps).await {
            Ok(receipt) => {
                self.last_rebalance = state::now_secs();
                let mut snap = self.state.snapshot.write().await;
                snap.rebalances += 1;
                snap.cum_realized_pnl_b += receipt.realized_pnl_b;
                snap.cum_cost_b += receipt.cost_b;
                drop(snap);
                self.state
                    .push_log(
                        "info",
                        format!(
                            "recentered -> [{:.4}, {:.4}] realized={:.2} cost={:.2}",
                            receipt.new_lower,
                            receipt.new_upper,
                            receipt.realized_pnl_b,
                            receipt.cost_b
                        ),
                    )
                    .await;
            }
            Err(e) => {
                self.state
                    .push_log("error", format!("recenter failed: {e}"))
                    .await;
            }
        }
    }
}
