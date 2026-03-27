mod extract;
mod handler;
mod session_mgr;
mod state;
mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::get;
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::state::AppState;

fn load_env() {
    let _ = dotenvy::dotenv_override();
}

#[tokio::main]
async fn main() {
    load_env();

    // -- Configuration from environment --
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let graph_path = std::env::var("GRAPH_PATH")
        .unwrap_or_else(|_| dirs_home().join(".supplementbot/graph").to_string_lossy().into());
    let static_dir = std::env::var("STATIC_DIR")
        .unwrap_or_else(|_| "/home/mpolly/supplementbot.com".to_string());
    let max_concurrent: usize = std::env::var("MAX_CONCURRENT_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let daily_cap: usize = std::env::var("DAILY_SESSION_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(13);
    let monthly_cap: usize = std::env::var("MONTHLY_SESSION_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    let session_timeout_secs: u64 = std::env::var("SESSION_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900); // 15 minutes
    let idisk_data_dir = std::env::var("IDISK_DATA_DIR").ok();

    eprintln!("supplementbot-web starting...");
    eprintln!("  graph: {graph_path}");
    eprintln!("  static: {static_dir}");
    if let Some(ref dir) = idisk_data_dir {
        eprintln!("  iDISK: {dir}");
    }
    eprintln!("  limits: {max_concurrent} concurrent, {daily_cap}/day, {monthly_cap}/month");
    eprintln!("  session timeout: {session_timeout_secs}s");

    // -- Initialize shared state --
    let state = AppState::init(
        &graph_path,
        idisk_data_dir.as_deref(),
        max_concurrent,
        daily_cap,
        monthly_cap,
        session_timeout_secs,
    )
    .await;

    // -- Routes --
    let app = Router::new()
        .route("/ws/chat", get(ws::ws_handler))
        .route("/api/health", get(handler::health))
        .route("/api/stats", get(handler::stats))
        .fallback_service(ServeDir::new(&static_dir))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // -- Start server --
    let addr: SocketAddr = format!("{host}:{port}").parse().unwrap();
    eprintln!("  listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
