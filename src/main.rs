//! Self-hosted CLMM market maker for Solana (Orca / Raydium) with a local
//! web dashboard. Runs the simulated venue out of the box.

mod config;
mod engine;
mod venues;
mod web;

use clap::Parser;
use engine::state::{AppHandle, Command, SharedState};
use engine::Engine;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(name = "sol-clmm-mm", about = "Solana CLMM market maker with local dashboard")]
struct Cli {
    /// Path to config file.
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let cfg = config::Config::load(&cli.config)?;

    let venue = venues::build(&cfg)?;
    let state = Arc::new(SharedState::new(
        venue.name().to_string(),
        cfg.dry_run,
        cfg.half_width_bps,
        cfg.rebalance_trigger_bps,
    ));

    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(32);
    let handle = AppHandle { state: state.clone(), cmd_tx };

    // Engine task.
    let engine = Engine::new(venue, state.clone(), cfg.clone(), cmd_rx);
    let engine_task = tokio::spawn(engine.run());

    // Web task.
    let web_task = tokio::spawn(web::serve(handle, cfg.web_port));

    tracing::info!("open http://127.0.0.1:{} to control the bot", cfg.web_port);

    tokio::select! {
        r = engine_task => { tracing::error!("engine exited: {:?}", r); }
        r = web_task => { tracing::error!("web exited: {:?}", r); }
    }
    Ok(())
}
