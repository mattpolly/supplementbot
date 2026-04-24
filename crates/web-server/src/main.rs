mod explore;
mod extract;
mod handler;
mod session_mgr;
mod state;
mod symptom_resolver;
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
    let db_url = std::env::var("DB_URL").unwrap_or_else(|_| "ws://localhost:8000".to_string());
    let db_user = std::env::var("DB_USER").unwrap_or_else(|_| "root".to_string());
    let db_pass = std::env::var("DB_PASS").expect("DB_PASS must be set");
    let static_dir = std::env::var("STATIC_DIR")
        .expect("STATIC_DIR must be set (path to static site files)");
    let max_concurrent: usize = std::env::var("MAX_CONCURRENT_SESSIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let daily_cap: usize = std::env::var("DAILY_SESSION_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let monthly_cap: usize = std::env::var("MONTHLY_SESSION_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let session_timeout_secs: u64 = std::env::var("SESSION_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900); // 15 minutes
    let ip_daily_cap: usize = std::env::var("IP_DAILY_SESSION_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3); // max 3 sessions per IP per day
    let idisk_data_dir = std::env::var("IDISK_DATA_DIR").ok();
    let suppkg_path = std::env::var("SUPPKG_PATH").ok();

    eprintln!("supplementbot-web starting...");
    eprintln!("  db: {db_url}");
    eprintln!("  static: {static_dir}");
    if let Some(ref dir) = idisk_data_dir {
        eprintln!("  iDISK: {dir}");
    }
    if let Some(ref p) = suppkg_path {
        eprintln!("  SuppKG: {p}");
    }
    eprintln!("  limits: {max_concurrent} concurrent, {daily_cap}/day, {monthly_cap}/month, {ip_daily_cap}/IP/day");
    eprintln!("  session timeout: {session_timeout_secs}s");

    // -- Initialize shared state --
    let state = AppState::init(
        &db_url,
        &db_user,
        &db_pass,
        idisk_data_dir.as_deref(),
        suppkg_path.as_deref(),
        max_concurrent,
        daily_cap,
        monthly_cap,
        session_timeout_secs,
        ip_daily_cap,
    )
    .await;

    // -- Routes --
    let app = Router::new()
        .route("/ws/chat", get(ws::ws_handler))
        .route("/api/health", get(handler::health))
        .route("/api/stats", get(handler::stats))
        // Graph explorer
        .route("/api/explore/graph/stats", get(explore::graph_stats))
        .route("/api/explore/graph/nodes", get(explore::graph_nodes))
        .route("/api/explore/graph/edges", get(explore::graph_edges))
        .route("/api/explore/graph/node-aliases", get(explore::graph_node_aliases))
        .route("/api/explore/graph/node-cuis", get(explore::graph_node_cuis))
        .route("/api/explore/graph/edge-sources", get(explore::graph_edge_sources))
        .route("/api/explore/graph/edge-citations", get(explore::graph_edge_citations))
        // Relational explorer
        .route("/api/explore/relational/stats", get(explore::relational_stats))
        .route("/api/explore/relational/intake-stages", get(explore::intake_stages))
        .route("/api/explore/relational/intake-archetypes", get(explore::intake_archetypes))
        .route("/api/explore/relational/intake-symptom-profiles", get(explore::intake_symptom_profiles))
        .route("/api/explore/relational/intake-questions", get(explore::intake_questions))
        .route("/api/explore/relational/intake-clusters", get(explore::intake_clusters))
        .route("/api/explore/relational/idisk-ingredients", get(explore::idisk_ingredients))
        .route("/api/explore/relational/idisk-drugs", get(explore::idisk_drugs))
        .route("/api/explore/relational/idisk-interactions", get(explore::idisk_interactions))
        .route("/api/explore/relational/idisk-adverse", get(explore::idisk_adverse))
        .fallback_service(ServeDir::new(&static_dir))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    // -- Periodic session cleanup (evict timed-out sessions every minute) --
    {
        let cleanup_state = state;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                let evicted = cleanup_state.inner.sessions.cleanup_expired().await;
                if evicted > 0 {
                    eprintln!("[session_mgr] cleanup: evicted {evicted} timed-out session(s)");
                }
            }
        });
    }

    // -- Start server --
    let addr: SocketAddr = format!("{host}:{port}").parse().unwrap();
    eprintln!("  listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}
