//! Local-only web server. Binds 127.0.0.1 exclusively. Serves the embedded
//! dashboard and a WebSocket that streams snapshots out and takes commands in.

use crate::engine::state::{AppHandle, Command};
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

pub async fn serve(handle: AppHandle, port: u16) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler))
        .with_state(handle);

    // 127.0.0.1 ONLY — never bind 0.0.0.0. The bot holds a key; the control
    // surface must not be reachable off-host.
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    tracing::info!("dashboard at http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    Html(DASHBOARD_HTML)
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(handle): State<AppHandle>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, handle))
}

async fn handle_socket(mut socket: WebSocket, handle: AppHandle) {
    let mut push = tokio::time::interval(Duration::from_millis(1000));
    loop {
        tokio::select! {
            // Stream snapshot to the browser once per second.
            _ = push.tick() => {
                let snap = handle.state.snapshot.read().await.clone();
                match serde_json::to_string(&snap) {
                    Ok(json) => {
                        if socket.send(Message::Text(json)).await.is_err() {
                            break; // client gone
                        }
                    }
                    Err(e) => tracing::error!("serialize snapshot: {e}"),
                }
            }
            // Receive commands from the browser.
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(txt))) => {
                        match serde_json::from_str::<Command>(&txt) {
                            Ok(cmd) => {
                                if handle.cmd_tx.send(cmd).await.is_err() {
                                    tracing::error!("engine channel closed");
                                    break;
                                }
                            }
                            Err(e) => tracing::warn!("bad command from ui: {e}"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => { tracing::warn!("ws error: {e}"); break; }
                    _ => {}
                }
            }
        }
    }
}
