use std::path::Path;
use std::sync::Arc;

use graph_service::graph::KnowledgeGraph;
use graph_service::intake::idisk::IdiskImporter;
use graph_service::intake::seed::seed_intake_graph;
use graph_service::intake::store::IntakeGraphStore;
use graph_service::merge::MergeStore;
use graph_service::source::SourceStore;
use intake_agent::safety::SafetyFilter;
use llm_client::provider::LlmProvider;

use crate::session_mgr::SessionManager;

// ---------------------------------------------------------------------------
// AppState — shared across all connections via axum's State extractor.
//
// The graph, source store, and merge store are loaded once at startup.
// LLM providers are configured from environment variables.
// Session manager handles creation, lookup, limits, and cleanup.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<AppStateInner>,
}

pub struct AppStateInner {
    pub graph: KnowledgeGraph,
    pub source: SourceStore,
    pub merge: MergeStore,
    /// The conversational LLM (expensive — Sonnet, Gemini Pro, etc.)
    pub renderer: Arc<dyn LlmProvider>,
    /// The extraction LLM (can be cheap — Haiku, Flash, etc.)
    /// For v1, may be the same instance as renderer.
    pub extractor: Arc<dyn LlmProvider>,
    pub sessions: SessionManager,
    pub safety_filter: SafetyFilter,
    /// Intake knowledge graph store (process graph for clinical interview).
    pub intake_store: IntakeGraphStore,
    /// iDISK importer (drug interactions, adverse reactions, mechanisms).
    pub idisk: IdiskImporter,
}

impl AppState {
    pub async fn init(
        graph_path: &str,
        idisk_data_dir: Option<&str>,
        max_concurrent: usize,
        daily_cap: usize,
        monthly_cap: usize,
        session_timeout_secs: u64,
    ) -> Self {
        // Open persistent graph
        let graph = KnowledgeGraph::open(graph_path)
            .await
            .expect("failed to open knowledge graph");

        let source = SourceStore::new(graph.db());
        let merge = MergeStore::new(graph.db());

        // Initialize intake KG services (same DB, intake_-prefixed tables)
        let intake_store = IntakeGraphStore::new(graph.db());
        let idisk = IdiskImporter::new(graph.db());

        // Seed intake graph structure (idempotent)
        seed_intake_graph(&intake_store).await;
        eprintln!("  intake KG seeded");

        // Import iDISK data if directory is provided and exists
        if let Some(dir) = idisk_data_dir {
            let path = Path::new(dir);
            if path.is_dir() {
                match idisk.import_all(path, &intake_store).await {
                    Ok(stats) => {
                        eprintln!(
                            "  iDISK loaded: {} symptoms, {} drugs, {} ingredients, {} adverse, {} interactions",
                            stats.symptom_profiles, stats.drugs, stats.ingredients,
                            stats.adverse_reactions, stats.interactions
                        );
                    }
                    Err(e) => {
                        eprintln!("  iDISK import failed: {e}");
                    }
                }
            } else {
                eprintln!("  iDISK dir not found: {dir} — skipping");
            }
        }

        // Configure LLM providers from environment
        let renderer = build_renderer();
        let extractor = build_extractor();

        let sessions = SessionManager::new(
            max_concurrent,
            daily_cap,
            monthly_cap,
            session_timeout_secs,
        );

        let safety_filter = SafetyFilter::new();

        let node_count = graph.node_count().await;
        let edge_count = graph.edge_count().await;
        eprintln!(
            "  graph loaded: {} nodes, {} edges",
            node_count, edge_count
        );
        eprintln!(
            "  renderer: {} ({})",
            renderer.provider_name(),
            renderer.model_name()
        );
        eprintln!(
            "  extractor: {} ({})",
            extractor.provider_name(),
            extractor.model_name()
        );

        Self {
            inner: Arc::new(AppStateInner {
                graph,
                source,
                merge,
                renderer,
                extractor,
                sessions,
                safety_filter,
                intake_store,
                idisk,
            }),
        }
    }
}

/// Build the conversational renderer LLM from RENDERER_PROVIDER / RENDERER_MODEL env vars.
/// Falls back to Anthropic Sonnet.
fn build_renderer() -> Arc<dyn LlmProvider> {
    let provider = std::env::var("RENDERER_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());
    let model = std::env::var("RENDERER_MODEL")
        .unwrap_or_else(|_| default_model_for(&provider).to_string());
    build_provider(&provider, &model)
}

/// Build the extraction LLM from EXTRACTOR_PROVIDER / EXTRACTOR_MODEL env vars.
/// Falls back to same as renderer.
fn build_extractor() -> Arc<dyn LlmProvider> {
    let provider = std::env::var("EXTRACTOR_PROVIDER")
        .or_else(|_| std::env::var("RENDERER_PROVIDER"))
        .unwrap_or_else(|_| "anthropic".to_string());
    let model = std::env::var("EXTRACTOR_MODEL")
        .or_else(|_| std::env::var("RENDERER_MODEL"))
        .unwrap_or_else(|_| default_model_for(&provider).to_string());
    build_provider(&provider, &model)
}

fn build_provider(provider: &str, model: &str) -> Arc<dyn LlmProvider> {
    match provider {
        "anthropic" => {
            use llm_client::anthropic::AnthropicProvider;
            Arc::new(
                AnthropicProvider::from_env(model)
                    .expect("ANTHROPIC_API_KEY required for anthropic provider"),
            )
        }
        "gemini" => {
            use llm_client::gemini::GeminiProvider;
            Arc::new(
                GeminiProvider::from_env(model)
                    .expect("GOOGLE_API_KEY required for gemini provider"),
            )
        }
        "xai" => {
            use llm_client::xai::XaiProvider;
            Arc::new(
                XaiProvider::from_env(model).expect("XAI_API_KEY required for xai provider"),
            )
        }
        other => panic!("unknown LLM provider: {other}"),
    }
}

fn default_model_for(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-sonnet-4-20250514",
        "gemini" => "gemini-2.5-pro-preview-05-06",
        "xai" => "grok-3",
        _ => "unknown",
    }
}
