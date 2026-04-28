use std::path::Path;
use std::sync::Arc;

use graph_service::graph::KnowledgeGraph;
use graph_service::intake::idisk::IdiskImporter;
use graph_service::intake::seed::seed_intake_graph;
use graph_service::intake::store::IntakeGraphStore;
use graph_service::merge::MergeStore;
use graph_service::query::{ArchetypeCoverage, QueryEngine};
use graph_service::source::SourceStore;
use intake_agent::safety::SafetyFilter;
use llm_client::provider::LlmProvider;
use suppkg::SuppKg;

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
    /// SuppKG — in-memory citation index (PubMed PMIDs + sentences).
    /// None if SUPPKG_PATH is not set or file not found.
    #[allow(dead_code)]
    pub suppkg: Option<Arc<SuppKg>>,
    /// Coverage strength per symptom archetype, sorted Strong → Moderate → Weak.
    /// Cached at startup for use in the opening greeting.
    pub archetype_coverage: Vec<ArchetypeCoverage>,
    /// If true, include the LLM system prompt in each WS response (DEBUG_LLM_PROMPT=true).
    pub debug_llm_prompt: bool,
}

impl AppState {
    pub async fn init(
        db_url: &str,
        db_user: &str,
        db_pass: &str,
        idisk_data_dir: Option<&str>,
        suppkg_path: Option<&str>,
        max_concurrent: usize,
        daily_cap: usize,
        monthly_cap: usize,
        session_timeout_secs: u64,
        ip_daily_cap: usize,
    ) -> Self {
        // Connect to SurrealDB server
        let graph = KnowledgeGraph::open(db_url, db_user, db_pass)
            .await
            .expect("failed to connect to knowledge graph database");

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

        // Load SuppKG citation index if path is provided
        let suppkg = suppkg_path.and_then(|p| {
            let path = Path::new(p);
            if path.exists() {
                match SuppKg::load(p) {
                    Ok(kg) => {
                        eprintln!("  SuppKG loaded: {} nodes, {} edge pairs", kg.node_count(), kg.edge_pair_count());
                        Some(Arc::new(kg))
                    }
                    Err(e) => {
                        eprintln!("  SuppKG load failed: {e}");
                        None
                    }
                }
            } else {
                eprintln!("  SuppKG path not found: {p} — citations unavailable");
                None
            }
        });

        // Configure LLM providers from environment
        let renderer = build_renderer();
        let extractor = build_extractor();

        let sessions = SessionManager::new(
            max_concurrent,
            daily_cap,
            monthly_cap,
            session_timeout_secs,
            ip_daily_cap,
        );

        let ingredient_names_path = std::env::var("INGREDIENT_NAMES_PATH").ok();
        let safety_filter = SafetyFilter::new(
            ingredient_names_path.as_deref().map(Path::new)
        );

        let node_count = graph.node_count().await;
        let edge_count = graph.edge_count().await;

        // Compute per-archetype coverage at startup (query-only, no DB writes)
        let query_engine = QueryEngine::new(&graph, &source, &merge).await;
        let archetypes = intake_store.all_archetypes().await;
        let archetype_coverage = query_engine.coverage_by_archetype(&archetypes).await;
        let strong_count = archetype_coverage.iter().filter(|c| c.strength == graph_service::query::CoverageStrength::Strong).count();
        let moderate_count = archetype_coverage.iter().filter(|c| c.strength == graph_service::query::CoverageStrength::Moderate).count();

        eprintln!("  graph loaded: {} nodes, {} edges", node_count, edge_count);
        eprintln!(
            "  coverage: {}/{} archetypes strong, {}/{} moderate",
            strong_count, archetype_coverage.len(),
            moderate_count, archetype_coverage.len()
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

        let debug_llm_prompt = std::env::var("DEBUG_LLM_PROMPT").map(|v| v == "true").unwrap_or(false);
        if debug_llm_prompt {
            eprintln!("  DEBUG_LLM_PROMPT=true — system prompts will be sent to client");
        }

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
                suppkg,
                archetype_coverage,
                debug_llm_prompt,
            }),
        }
    }
}

/// Build the conversational renderer LLM from RENDERER_PROVIDER / RENDERER_MODEL env vars.
/// If GOOGLE_API_KEY is set, wraps with a Gemini fallback and 20s timeout.
fn build_renderer() -> Arc<dyn LlmProvider> {
    let provider = std::env::var("RENDERER_PROVIDER").unwrap_or_else(|_| "anthropic".to_string());
    let model = std::env::var("RENDERER_MODEL")
        .unwrap_or_else(|_| default_model_for(&provider).to_string());
    let primary = build_provider(&provider, &model);
    let gemini_model = std::env::var("GEMINI_MODEL")
        .unwrap_or_else(|_| "gemini-2.0-flash".to_string());
    wrap_with_fallback(primary, "gemini", &gemini_model)
}

/// Build the extraction LLM from EXTRACTOR_PROVIDER / EXTRACTOR_MODEL env vars.
/// Wraps with a Gemini fallback and 20s timeout if GEMINI_API_KEY is set.
fn build_extractor() -> Arc<dyn LlmProvider> {
    let provider = std::env::var("EXTRACTOR_PROVIDER")
        .or_else(|_| std::env::var("RENDERER_PROVIDER"))
        .unwrap_or_else(|_| "anthropic".to_string());
    let model = std::env::var("EXTRACTOR_MODEL")
        .or_else(|_| std::env::var("RENDERER_MODEL"))
        .unwrap_or_else(|_| default_model_for(&provider).to_string());
    let primary = build_provider(&provider, &model);
    let gemini_model = std::env::var("GEMINI_MODEL")
        .unwrap_or_else(|_| "gemini-2.0-flash".to_string());
    wrap_with_fallback(primary, "gemini", &gemini_model)
}

/// Wrap a provider with a Gemini fallback if GOOGLE_API_KEY is available.
/// Uses a 20-second timeout per call. If the key isn't set, returns primary as-is.
fn wrap_with_fallback(
    primary: Arc<dyn LlmProvider>,
    fallback_provider: &str,
    fallback_model: &str,
) -> Arc<dyn LlmProvider> {
    use llm_client::fallback::FallbackProvider;
    use std::time::Duration;

    if let Ok(fallback) = try_build_provider(fallback_provider, fallback_model) {
        eprintln!(
            "  [fallback] {}/{} → {}/{}  (20s timeout)",
            primary.provider_name(), primary.model_name(),
            fallback_provider, fallback_model
        );
        Arc::new(FallbackProvider::new(primary, fallback, Duration::from_secs(20)))
    } else {
        primary
    }
}

/// Try to build a provider without panicking on missing keys.
fn try_build_provider(provider: &str, model: &str) -> Result<Arc<dyn LlmProvider>, ()> {
    match provider {
        "gemini" => {
            use llm_client::gemini::GeminiProvider;
            GeminiProvider::from_env(model)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|_| ())
        }
        "anthropic" => {
            use llm_client::anthropic::AnthropicProvider;
            AnthropicProvider::from_env(model)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|_| ())
        }
        _ => Err(()),
    }
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
