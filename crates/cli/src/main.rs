use clap::{Parser, Subcommand};
use event_log::sink::{EventSink, JsonlFileSink};
use graph_service::graph::KnowledgeGraph;
use graph_service::lens::ComplexityLens;
use graph_service::merge::MergeStore;
use graph_service::query::{QueryConfig, QueryEngine};
use graph_service::source::{EdgeQuality, SourceStore};
use llm_client::anthropic::AnthropicProvider;
use llm_client::gemini::GeminiProvider;
use llm_client::mock::MockProvider;
use llm_client::provider::LlmProvider;
use llm_client::xai::XaiProvider;
use nsai_loop::loop_runner::NsaiLoop;
use suppkg::SuppKg;
use uuid::Uuid;

// Load .env file before anything reads environment variables.
// Uses dotenv_override so .env always wins over shell exports.
fn load_env() {
    if let Err(_) = dotenvy::dotenv_override() {
        // No .env file is fine — keys can come from the shell environment
    }
}

#[derive(Parser)]
#[command(name = "supplementbot")]
#[command(about = "Neurosymbolic AI for systemic wellness — NSAI loop runner")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Nutraceutical(s) to study, comma-separated (e.g. "Magnesium,Zinc")
    #[arg(short, long, value_delimiter = ',')]
    nutraceutical: Vec<String>,

    /// LLM provider: anthropic, gemini, or mock
    #[arg(short, long, default_value = "mock")]
    provider: String,

    /// Model name (provider-specific). Defaults vary by provider.
    #[arg(short, long)]
    model: Option<String>,

    /// Output file for the event log (JSONL format)
    #[arg(short, long, default_value = "events.jsonl")]
    output: String,

    /// Max gap-filling iterations per grade level
    #[arg(long, default_value = "3")]
    max_iterations: u32,

    /// Max gaps to fill per iteration
    #[arg(long, default_value = "5")]
    max_gaps: usize,

    /// Max structural observations to validate speculatively (0 = skip)
    #[arg(long, default_value = "3")]
    max_speculative: usize,

    /// Export the graph as JSON + HTML for visualization and exit
    #[arg(long, value_name = "PATH")]
    export: Option<String>,

    /// Enable SuppKG synonym resolution (default: data/ dir, or specify a custom dir)
    #[arg(long, value_name = "DIR", default_missing_value = "data", num_args = 0..=1)]
    suppkg: Option<String>,

    /// Complexity lens for extraction: 5th, 10th, college, graduate, or 0.0-1.0
    #[arg(short, long, default_value = "5th")]
    lens: String,

    /// Run citation backing only — no LLM calls. Requires SuppKG (--suppkg or SUPPKG_PATH env).
    #[arg(long)]
    cite_only: bool,

    /// Resolve CUIs for all graph nodes from SuppKG, then exit — no LLM calls.
    /// Run this before --cite-only if the graph was built without --suppkg.
    #[arg(long)]
    resolve_cuis: bool,

    /// Populate the ingredient registry from external data sources
    /// (iDISK, supplement_cuis.jsonl, CTD). Specify the data directory
    /// containing idisk2/, ctd/, and supplement_cuis.jsonl.
    #[arg(long, value_name = "DIR")]
    hydrate_registry: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Query the knowledge graph
    Query {
        #[command(subcommand)]
        query_type: QueryType,

        /// Complexity level: 5th, 10th, college, graduate, or a 0.0-1.0 value
        #[arg(short, long, default_value = "5th")]
        lens: String,

        /// Minimum quality tier: deduced, speculative, single, multi, citation
        #[arg(short, long)]
        quality: Option<String>,

        /// Minimum edge confidence (0.0-1.0)
        #[arg(short, long)]
        confidence: Option<f64>,
    },

    /// Migrate data from an embedded RocksDB graph to the SurrealDB server.
    /// Destination is configured via DB_URL / DB_USER / DB_PASS env vars.
    Migrate {
        /// Path to the existing embedded RocksDB graph directory
        #[arg(long)]
        from: String,
    },

    /// Confirm graph edges against supplementology evidence claims.
    ///
    /// Queries the supplementology API for evidence claims per ingredient,
    /// matches them against existing graph edges, and inserts CitationRecords
    /// into edge_citation. This promotes matched edges to CitationBacked
    /// quality (1.5x score multiplier) automatically.
    ///
    /// Run once after supplementology Phase 6 completes, then re-run whenever
    /// new research is ingested into supplementology.
    ConfirmEdges {
        /// Only confirm edges for this ingredient (e.g. "magnesium").
        /// Omit to run for all ingredients in the graph.
        #[arg(long)]
        ingredient: Option<String>,

        /// Supplementology API base URL
        #[arg(long, default_value = "http://localhost:3001")]
        supplementology_url: String,
    },
}

#[derive(Subcommand)]
enum QueryType {
    /// What ingredients address this symptom?
    Symptom {
        /// The symptom to look up (e.g. "muscle cramps")
        name: String,
    },
    /// What ingredients act on this system?
    System {
        /// The body system to look up (e.g. "nervous system")
        name: String,
    },
    /// What does this ingredient do?
    Ingredient {
        /// The ingredient to look up (e.g. "magnesium")
        name: String,
    },
    /// List all ingredients the graph has been trained on
    List,
}

fn db_connection() -> (String, String, String) {
    let url = std::env::var("DB_URL").unwrap_or_else(|_| "ws://localhost:8000".to_string());
    let user = std::env::var("DB_USER").unwrap_or_else(|_| "root".to_string());
    let pass = std::env::var("DB_PASS").unwrap_or_else(|_| {
        eprintln!("Warning: DB_PASS not set, using empty password");
        String::new()
    });
    (url, user, pass)
}

fn build_provider(cli: &Cli) -> Result<Box<dyn LlmProvider>, String> {
    match cli.provider.as_str() {
        "anthropic" => {
            let model = cli
                .model
                .clone()
                .or_else(|| std::env::var("ANTHROPIC_MODEL").ok())
                .unwrap_or_else(|| "claude-sonnet-4-6".into());
            AnthropicProvider::from_env(model)
                .map(|p| Box::new(p) as Box<dyn LlmProvider>)
                .map_err(|e| e.to_string())
        }
        "gemini" => {
            let model = cli
                .model
                .clone()
                .or_else(|| std::env::var("GEMINI_MODEL").ok())
                .unwrap_or_else(|| "gemini-flash-latest".into());
            GeminiProvider::from_env(model)
                .map(|p| Box::new(p) as Box<dyn LlmProvider>)
                .map_err(|e| e.to_string())
        }
        "grok" | "xai" => {
            let model = cli
                .model
                .clone()
                .or_else(|| std::env::var("XAI_MODEL").ok())
                .unwrap_or_else(|| "grok-4-1-fast-reasoning".into());
            XaiProvider::from_env(model)
                .map(|p| Box::new(p) as Box<dyn LlmProvider>)
                .map_err(|e| e.to_string())
        }
        "mock" => {
            let provider = MockProvider::new("mock", "mock-v1")
                // Seed answer (5th grade)
                .on(
                    "5th grader",
                    "Magnesium helps your muscles relax, helps you sleep better, \
                     and gives your body energy.",
                )
                // Extraction of seed
                .on(
                    "muscles relax",
                    "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                     magnesium|Ingredient|acts_on|muscular system|System\n\
                     magnesium|Ingredient|affords|sleep quality|Property",
                )
                .on(
                    "sleep better",
                    "magnesium|Ingredient|affords|sleep quality|Property",
                )
                .on(
                    "energy",
                    "magnesium|Ingredient|affords|energy production|Property\n\
                     magnesium|Ingredient|via_mechanism|atp synthesis|Mechanism",
                )
                // Gap-fill answers
                .on(
                    "connected to muscle relaxation",
                    "Magnesium helps muscles relax by stopping them from staying tight.",
                )
                .on(
                    "staying tight",
                    "magnesium|Ingredient|via_mechanism|muscle tension relief|Mechanism\n\
                     muscle tension relief|Mechanism|affords|muscle relaxation|Property",
                )
                .on(
                    "help with sleep quality",
                    "Magnesium helps calm your brain so you can fall asleep.",
                )
                .on(
                    "calm your brain",
                    "magnesium|Ingredient|acts_on|nervous system|System\n\
                     magnesium|Ingredient|affords|sleep quality|Property",
                )
                .on(
                    "connected to energy production",
                    "Magnesium helps your body turn food into energy.",
                )
                .on(
                    "turn food into energy",
                    "magnesium|Ingredient|via_mechanism|energy metabolism|Mechanism\n\
                     energy metabolism|Mechanism|affords|energy production|Property",
                )
                // Comprehension rephrase
                .on(
                    "explain the same",
                    "Magnesium keeps your muscles from getting too tight \
                     and helps your brain relax for sleep.",
                )
                // Extraction of rephrase
                .on(
                    "too tight",
                    "magnesium|Ingredient|affords|muscle relaxation|Property\n\
                     magnesium|Ingredient|affords|sleep quality|Property",
                )
                // Catch-all
                .with_default(
                    "magnesium|Ingredient|modulates|cellular function|Mechanism",
                );
            Ok(Box::new(provider))
        }
        other => Err(format!(
            "Unknown provider: {}. Use: anthropic, gemini, grok, mock",
            other
        )),
    }
}

fn parse_lens(s: &str) -> ComplexityLens {
    match s {
        "5th" => ComplexityLens::fifth_grade(),
        "10th" => ComplexityLens::tenth_grade(),
        "college" => ComplexityLens::college(),
        "graduate" => ComplexityLens::graduate(),
        other => {
            if let Ok(v) = other.parse::<f64>() {
                ComplexityLens::new(v)
            } else {
                eprintln!("Unknown lens '{}', using 5th grade", other);
                ComplexityLens::fifth_grade()
            }
        }
    }
}

fn parse_quality(s: &str) -> Option<EdgeQuality> {
    match s {
        "deduced" => Some(EdgeQuality::Deduced),
        "speculative" => Some(EdgeQuality::Speculative),
        "single" => Some(EdgeQuality::SingleProvider),
        "multi" => Some(EdgeQuality::MultiProvider),
        "citation" => Some(EdgeQuality::CitationBacked),
        other => {
            eprintln!("Unknown quality tier '{}', ignoring filter", other);
            None
        }
    }
}

async fn run_migrate(from: &str) {
    println!("─── Migrate ────────────────────────────────────────────");
    println!("  Source (embedded): {}", from);

    let src = match KnowledgeGraph::open_embedded(from).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error opening embedded graph: {}", e);
            std::process::exit(1);
        }
    };

    let src_nodes = src.node_count().await;
    let src_edges = src.edge_count().await;
    println!("  Found: {} nodes, {} edges", src_nodes, src_edges);

    if src_nodes == 0 {
        eprintln!("Source graph is empty — nothing to migrate.");
        std::process::exit(1);
    }

    let (db_url, db_user, db_pass) = db_connection();
    println!("  Destination (server): {}", db_url);

    let dst = match KnowledgeGraph::open(&db_url, &db_user, &db_pass).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error connecting to destination server: {}", e);
            std::process::exit(1);
        }
    };

    let dst_nodes_before = dst.node_count().await;
    if dst_nodes_before > 0 {
        eprintln!(
            "Warning: destination already has {} nodes. Proceeding will add to existing data.",
            dst_nodes_before
        );
    }

    // --- Nodes ---
    let src_node_list = src.all_nodes().await;
    let mut nodes_inserted = 0usize;
    for idx in &src_node_list {
        if let Some(data) = src.node_data(idx).await {
            dst.add_node(data).await;
            nodes_inserted += 1;
        }
    }
    println!("  node: {} / {} inserted", nodes_inserted, src_node_list.len());

    // --- Edges (use typed API to avoid RecordId serialization issues) ---
    let src_edges_list = src.all_edges().await;
    let edge_count = src_edges_list.len();
    let mut edges_inserted = 0usize;
    for (src_idx, tgt_idx, edge_data) in src_edges_list {
        // Resolve node names from source, find them in destination
        if let (Some(src_data), Some(tgt_data)) = (
            src.node_data(&src_idx).await,
            src.node_data(&tgt_idx).await,
        ) {
            if let (Some(dst_src), Some(dst_tgt)) = (
                dst.find_node(&src_data.name).await,
                dst.find_node(&tgt_data.name).await,
            ) {
                dst.add_edge(&dst_src, &dst_tgt, edge_data).await;
                edges_inserted += 1;
            }
        }
    }
    println!("  edge: {} / {} inserted", edges_inserted, edge_count);

    // --- Flat tables (source/merge) via raw JSON ---
    let flat_tables = ["node_source", "edge_source", "node_alias", "node_cui"];
    for table in &flat_tables {
        let records: Vec<serde_json::Value> = src
            .db()
            .query(format!("SELECT * FROM {table}"))
            .await
            .and_then(|mut r| r.take(0))
            .unwrap_or_default();

        let count = records.len();
        if count == 0 {
            println!("  {}: empty, skipping", table);
            continue;
        }

        let mut inserted = 0usize;
        for record in &records {
            let ok = dst
                .db()
                .create::<Option<serde_json::Value>>(*table)
                .content(record.clone())
                .await
                .is_ok();
            if ok {
                inserted += 1;
            }
        }
        println!("  {}: {} / {} inserted", table, inserted, count);
    }

    let dst_nodes = dst.node_count().await;
    let dst_edges = dst.edge_count().await;
    println!();
    println!("  Done: {} nodes, {} edges in destination", dst_nodes, dst_edges);
    println!("  Verify with: supplementbot query list");
}

async fn run_confirm_edges(ingredient: Option<String>, supplementology_url: String) {
    let (db_url, db_user, db_pass) = db_connection();

    let graph = KnowledgeGraph::open(&db_url, &db_user, &db_pass).await
        .expect("failed to connect to graph DB");
    let source_store = SourceStore::new(graph.db());

    // Get ingredient names to process
    let all_ingredients = graph.known_ingredients().await;
    let ingredients: Vec<String> = if let Some(ref name) = ingredient {
        all_ingredients.into_iter()
            .filter(|n| n.to_lowercase() == name.to_lowercase())
            .collect()
    } else {
        all_ingredients
    };

    println!("─── Confirm Edges ──────────────────────────────────────");
    println!("  Supplementology: {}", supplementology_url);
    println!("  Ingredients:     {}", ingredients.len());
    println!();

    let client = reqwest::Client::new();
    let mut total_matched = 0usize;
    let mut total_new = 0usize;

    for ingredient_name in &ingredients {
        let slug = ingredient_name.to_lowercase().replace(' ', "_").replace('-', "_");

        let url = format!("{}/v1/graph-feed/{}", supplementology_url, slug);
        let resp = match client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            Ok(r) if r.status() == 404 => continue, // no supplementology data yet
            Ok(r) => { eprintln!("  {} → HTTP {}", ingredient_name, r.status()); continue; }
            Err(e) => { eprintln!("  {} → {}", ingredient_name, e); continue; }
        };

        let feed: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => { eprintln!("  {} → parse error: {}", ingredient_name, e); continue; }
        };

        // Get all outgoing edges for this ingredient
        let ingredient_idx = match graph.find_node(ingredient_name).await {
            Some(idx) => idx,
            None => continue,
        };
        let outgoing = graph.outgoing_edges(&ingredient_idx).await;
        if outgoing.is_empty() { continue; }

        // Resolve target names for all edges (one DB call each)
        let mut edge_info: Vec<(String, String)> = Vec::new(); // (target_name_lower, edge_type)
        for (tidx, edata) in &outgoing {
            let tname = graph.node_data(tidx).await
                .map(|d| d.name.to_lowercase())
                .unwrap_or_default();
            edge_info.push((tname, edata.edge_type.to_string()));
        }

        let mut ingredient_new = 0usize;

        // Match evidence claims → graph edges
        if let Some(claims) = feed["evidence_claims"].as_array() {
            for claim in claims {
                let pmid = match claim["citation"]["pmid"].as_str() {
                    Some(p) if !p.is_empty() => p.to_string(),
                    _ => continue,
                };
                let claim_text = claim["claim_text"].as_str().unwrap_or("").to_lowercase();
                let outcome   = claim["outcome"].as_str().unwrap_or("").to_lowercase();
                let mechanism = claim["mechanism"].as_str().unwrap_or("").to_lowercase();
                let direction = claim["direction"].as_str().unwrap_or("neutral");
                let confidence = claim["confidence"].as_f64().unwrap_or(0.7);

                for (i, (target_lower, edge_type)) in edge_info.iter().enumerate() {
                    if target_lower.is_empty() { continue; }
                    // Skip direction mismatch
                    if edge_type == "has_adverse_reaction" && direction == "positive" { continue; }

                    let hit = outcome.contains(target_lower.as_str())
                        || mechanism.contains(target_lower.as_str())
                        || claim_text.contains(target_lower.as_str());

                    if hit {
                        let target_name = graph.node_data(&outgoing[i].0).await
                            .map(|d| d.name)
                            .unwrap_or_default();
                        let record = graph_service::source::CitationRecord {
                            source_node: ingredient_name.clone(),
                            target_node: target_name,
                            edge_type: edge_type.clone(),
                            pmid: pmid.clone(),
                            sentence: claim["claim_text"].as_str().unwrap_or("").to_string(),
                            confidence,
                            suppkg_predicate: "supplementology".to_string(),
                            source_cui: String::new(),
                            target_cui: String::new(),
                        };
                        total_matched += 1;
                        if source_store.record_citation(&record).await {
                            ingredient_new += 1;
                        }
                        break; // one citation per claim
                    }
                }
            }
        }

        // Also handle effectiveness relationships from iDISK/CTD
        if let Some(effectiveness) = feed["effectiveness"].as_array() {
            for eff in effectiveness {
                let cond_lower = eff["condition_name"].as_str().unwrap_or("").to_lowercase();
                for (i, (target_lower, edge_type)) in edge_info.iter().enumerate() {
                    if edge_type != "is_effective_for" { continue; }
                    if target_lower.contains(&cond_lower) || cond_lower.contains(target_lower.as_str()) {
                        let target_name = graph.node_data(&outgoing[i].0).await
                            .map(|d| d.name)
                            .unwrap_or_default();
                        let record = graph_service::source::CitationRecord {
                            source_node: ingredient_name.clone(),
                            target_node: target_name,
                            edge_type: edge_type.clone(),
                            pmid: format!("suppl:{}/{}", slug, cond_lower.replace(' ', "_")),
                            sentence: format!("{} is effective for {}",
                                ingredient_name,
                                eff["condition_name"].as_str().unwrap_or("")),
                            confidence: eff["confidence"].as_f64().unwrap_or(0.8),
                            suppkg_predicate: "supplementology_effectiveness".to_string(),
                            source_cui: String::new(),
                            target_cui: String::new(),
                        };
                        total_matched += 1;
                        if source_store.record_citation(&record).await {
                            ingredient_new += 1;
                        }
                        break;
                    }
                }
            }
        }

        if ingredient_new > 0 {
            println!("  {:25} → {} new citations stored", ingredient_name, ingredient_new);
        }
        total_new += ingredient_new;
    }

    println!();
    println!("  Edges matched       : {}", total_matched);
    println!("  New citations stored: {}", total_new);
    println!("  Edges with stored citations are now CitationBacked (1.5× score multiplier)");
    println!("  Re-run after new supplementology imports to pick up fresh evidence.");
}

async fn run_query(
    query_type: QueryType,
    lens_str: String,
    quality_str: Option<String>,
    min_confidence: Option<f64>,
) {
    let (db_url, db_user, db_pass) = db_connection();
    let graph = match KnowledgeGraph::open(&db_url, &db_user, &db_pass).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error opening graph database: {}", e);
            std::process::exit(1);
        }
    };

    let node_count = graph.node_count().await;
    if node_count == 0 {
        eprintln!("Graph is empty. Run an ingestion first.");
        std::process::exit(1);
    }

    let source = SourceStore::new(graph.db());
    let merge = MergeStore::new(graph.db());
    let engine = QueryEngine::new(&graph, &source, &merge).await;

    let lens = parse_lens(&lens_str);
    let min_quality = quality_str.and_then(|s| parse_quality(&s));

    let config = QueryConfig {
        lens,
        min_quality,
        min_confidence,
        ..Default::default()
    };

    println!("  Graph:  {} nodes, {} edges", node_count, graph.edge_count().await);
    println!("  Lens:   {} ({})", lens_str, config.lens.level());
    println!();

    match query_type {
        QueryType::List => {
            let ingredients = graph.known_ingredients().await;
            if ingredients.is_empty() {
                println!("  No ingredients found. Run an ingestion first.");
            } else {
                println!("─── Known Ingredients ({}) ──────────────────────────", ingredients.len());
                println!();
                for name in &ingredients {
                    println!("  • {}", name);
                }
                println!();
            }
            return;
        }

        QueryType::Symptom { name } => {
            println!("─── Ingredients for symptom: {} ─────────────────────", name);
            println!();

            let results = engine.ingredients_for_symptom(&name, &config).await;

            if results.is_empty() {
                println!("  No results found.");
                println!();
                println!("  Possible reasons:");
                println!("    - Symptom '{}' is not in the graph", name);
                println!("    - No ingredients connect to it at this lens/quality level");
                return;
            }

            for (i, result) in results.iter().enumerate() {
                println!(
                    "  {}. {} (score: {:.3}, quality: {})",
                    i + 1,
                    result.ingredient.name,
                    result.best_score,
                    result.weakest_quality.label(),
                );

                for (j, path) in result.paths.iter().enumerate() {
                    let route = path.explanation.join(" → ");
                    println!("     path {}: {} (score: {:.3})", j + 1, route, path.score);
                }

                if !result.contraindications.is_empty() {
                    for contra in &result.contraindications {
                        let desc = contra.explanation.join(" → ");
                        println!("     ⚠ CONTRAINDICATION: {}", desc);
                    }
                }
                println!();
            }
        }

        QueryType::System { name } => {
            println!("─── Ingredients for system: {} ──────────────────────", name);
            println!();

            let results = engine.ingredients_for_system(&name, &config).await;

            if results.is_empty() {
                println!("  No results found.");
                return;
            }

            for (i, result) in results.iter().enumerate() {
                println!(
                    "  {}. {} (score: {:.3}, quality: {})",
                    i + 1,
                    result.ingredient.name,
                    result.best_score,
                    result.weakest_quality.label(),
                );

                for (j, path) in result.paths.iter().enumerate() {
                    let route = path.explanation.join(" → ");
                    println!("     path {}: {} (score: {:.3})", j + 1, route, path.score);
                }

                if !result.contraindications.is_empty() {
                    for contra in &result.contraindications {
                        let desc = contra.explanation.join(" → ");
                        println!("     ⚠ CONTRAINDICATION: {}", desc);
                    }
                }
                println!();
            }
        }

        QueryType::Ingredient { name } => {
            println!("─── Effects of ingredient: {} ───────────────────────", name);
            println!();

            let results = engine.effects_of_ingredient(&name, &config).await;

            if results.is_empty() {
                println!("  No results found.");
                return;
            }

            for (i, result) in results.iter().enumerate() {
                println!(
                    "  {}. {} ({:?}, score: {:.3}, quality: {})",
                    i + 1,
                    result.destination.name,
                    result.destination.node_type,
                    result.best_score,
                    result.weakest_quality.label(),
                );

                for (j, path) in result.paths.iter().enumerate() {
                    let route = path.explanation.join(" → ");
                    println!("     path {}: {} (score: {:.3})", j + 1, route, path.score);
                }
                println!();
            }
        }
    }
}

#[tokio::main]
async fn main() {
    load_env();
    let cli = Cli::parse();

    // Handle subcommands
    match cli.command {
        Some(Commands::Query { query_type, lens, quality, confidence }) => {
            run_query(query_type, lens, quality, confidence).await;
            return;
        }
        Some(Commands::Migrate { from }) => {
            run_migrate(&from).await;
            return;
        }
        Some(Commands::ConfirmEdges { ingredient, supplementology_url }) => {
            run_confirm_edges(ingredient, supplementology_url).await;
            return;
        }
        None => {}
    }

    let nutraceuticals: Vec<String> = cli.nutraceutical.iter().map(|s| s.trim().to_string()).collect();

    let (db_path, db_user, db_pass) = db_connection();

    let ingestion_lens = parse_lens(&cli.lens);

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║      supplementbot — NSAI loop                      ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();
    println!("  Nutraceuticals: {}", nutraceuticals.join(", "));
    println!("  Provider:       {}", cli.provider);
    println!("  Lens:           {} ({})", cli.lens, ingestion_lens.level());
    println!("  Event log:      {}", cli.output);
    println!("  Graph DB:       {}", db_path);  // db_path is now the URL
    println!("  Max iterations: {}", cli.max_iterations);
    println!("  Max gaps/iter:  {}", cli.max_gaps);
    println!();

    let provider = match build_provider(&cli) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    println!("  Model:          {}", provider.model_name());
    println!();

    let sink = match JsonlFileSink::new(&cli.output) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error creating event log: {}", e);
            std::process::exit(1);
        }
    };

    // Connect to SurrealDB server
    let graph = match KnowledgeGraph::open(&db_path, &db_user, &db_pass).await {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Error opening graph database: {}", e);
            std::process::exit(1);
        }
    };

    let existing_nodes = graph.node_count().await;
    let existing_edges = graph.edge_count().await;
    if existing_nodes > 0 {
        println!("  Loaded existing graph: {} nodes, {} edges", existing_nodes, existing_edges);
        println!();
    }

    // Export and exit if requested
    if let Some(export_path) = &cli.export {
        let export = graph.export_json().await;
        let json = serde_json::to_string_pretty(&export).expect("Failed to serialize graph");

        std::fs::write(export_path, &json).expect("Failed to write JSON");

        let html_path = if export_path.ends_with(".json") {
            export_path.replace(".json", ".html")
        } else {
            format!("{}.html", export_path)
        };

        let html = graph_service::export::D3_HTML_TEMPLATE
            .replace(
                r#"/*__GRAPH_DATA__*/{"nodes":[],"edges":[]}"#,
                &json,
            );
        std::fs::write(&html_path, html).expect("Failed to write HTML");

        println!("  Exported {} nodes, {} edges", export.nodes.len(), export.edges.len());
        println!("  JSON: {}", export_path);
        println!("  HTML: {}", html_path);
        println!();
        println!("  Open the HTML file in a browser to visualize the graph.");
        return;
    }

    // Create source store for provenance tracking (shares DB with graph)
    let source_store = SourceStore::new(graph.db());

    // Create merge store for synonym resolution (shares DB with graph)
    let merge_store = MergeStore::new(graph.db());

    // --resolve-cuis: populate merge store CUIs from SuppKG and exit (no LLM)
    if cli.resolve_cuis {
        let suppkg_dir = cli.suppkg.clone()
            .or_else(|| {
                std::env::var("SUPPKG_PATH").ok().map(|p| {
                    std::path::Path::new(&p)
                        .parent()
                        .map(|d| d.to_string_lossy().to_string())
                        .unwrap_or(p)
                })
            });

        let dir = match suppkg_dir {
            Some(d) => d,
            None => {
                eprintln!("--resolve-cuis requires SuppKG. Pass --suppkg <dir> or set SUPPKG_PATH.");
                std::process::exit(1);
            }
        };

        let json_path = format!("{}/supp_kg.json", dir);
        let edgelist_path = format!("{}/suppkg_v2.edgelist", dir);

        print!("  Loading SuppKG from {}/ ...", dir);
        let kg_result = if std::path::Path::new(&edgelist_path).exists() {
            SuppKg::load_with_edgelist(&json_path, &edgelist_path)
        } else {
            SuppKg::load(&json_path)
        };

        let kg = match kg_result {
            Ok(kg) => {
                println!(
                    " {} nodes, {} terms, {} edge pairs",
                    kg.node_count(), kg.term_count(), kg.edge_pair_count()
                );
                kg
            }
            Err(e) => {
                eprintln!(" Error loading SuppKG: {}", e);
                std::process::exit(1);
            }
        };

        let sink = match JsonlFileSink::new(&cli.output) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error creating event log: {}", e);
                std::process::exit(1);
            }
        };

        // Derive supplement_cuis.jsonl path alongside events.jsonl
        let cache_path = std::path::Path::new(&cli.output)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("supplement_cuis.jsonl");

        let umls_api_key = std::env::var("UMLS_API_KEY").ok();
        if umls_api_key.is_some() {
            println!("  UMLS API key found — will resolve missing CUIs via API");
            println!("  CUI cache: {}", cache_path.display());
        } else {
            println!("  No UMLS_API_KEY — skipping API fallback for unresolved nodes");
        }

        println!("  Resolving CUIs for all graph nodes ...");
        let corr = Uuid::new_v4();
        let result = nsai_loop::synonym::run_synonym_resolution(
            &graph,
            &kg,
            &merge_store,
            corr,
            &sink,
            umls_api_key.as_deref(),
            Some(&cache_path),
        ).await;

        println!("─── CUI Resolution ─────────────────────────────────────");
        println!(
            "  {} CUIs assigned, {} aliases detected, {} via UMLS API",
            result.cuis_assigned, result.aliases_found, result.umls_api_calls
        );
        println!();

        sink.flush().expect("Failed to flush event log");
        return;
    }

    // --hydrate-registry: populate ingredient registry from external data and exit
    if let Some(ref data_dir) = cli.hydrate_registry {
        use graph_service::registry::{IngredientRecord, IngredientRegistry};

        let registry = IngredientRegistry::new(graph.db());
        let before = registry.count().await;

        println!("  Hydrating ingredient registry from {} ...", data_dir);

        // Collect all ingredient names currently in the graph
        let ingredient_nodes = graph.nodes_by_type(&graph_service::types::NodeType::Ingredient).await;
        let mut ingredient_names: Vec<String> = Vec::new();
        for idx in &ingredient_nodes {
            if let Some(nd) = graph.node_data(idx).await {
                ingredient_names.push(nd.name.to_lowercase());
            }
        }
        println!("  Found {} ingredients in graph", ingredient_names.len());

        // ── Load iDISK DSI.csv ──────────────────────────────────────────
        let idisk_path = format!("{}/idisk2/Entity/DSI.csv", data_dir);
        let mut idisk_map: std::collections::HashMap<String, (String, String, Vec<String>, String)> =
            std::collections::HashMap::new(); // name -> (idisk_id, cui, common_names, moa)

        if let Ok(mut rdr) = csv::Reader::from_path(&idisk_path) {
            for result in rdr.records() {
                if let Ok(record) = result {
                    let idisk_id = record.get(0).unwrap_or("").to_string();
                    let name = record.get(1).unwrap_or("").to_lowercase();
                    let cui = record.get(2).unwrap_or("").to_string();
                    let common_names_raw = record.get(7).unwrap_or("");
                    let common_names: Vec<String> = common_names_raw
                        .split('|')
                        .map(|s| s.trim().to_lowercase())
                        .filter(|s| !s.is_empty() && s.len() > 1)
                        .collect();
                    let moa = record.get(5).unwrap_or("").to_string(); // Mechanism of action

                    // Index by name and common names
                    idisk_map.insert(name.clone(), (idisk_id.clone(), cui.clone(), common_names.clone(), moa.clone()));
                    for cn in &common_names {
                        idisk_map.entry(cn.clone())
                            .or_insert((idisk_id.clone(), cui.clone(), common_names.clone(), moa.clone()));
                    }
                }
            }
            println!("  Loaded {} iDISK entries", idisk_map.len());
        } else {
            println!("  Warning: could not read {}", idisk_path);
        }

        // ── Load supplement_cuis.jsonl ──────────────────────────────────
        let cuis_path = format!("{}/supplement_cuis.jsonl", data_dir);
        let mut umls_map: std::collections::HashMap<String, (String, Vec<String>)> =
            std::collections::HashMap::new(); // name -> (cui, synonyms)

        if let Ok(content) = std::fs::read_to_string(&cuis_path) {
            for line in content.lines() {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                    let ingredient = val["ingredient"].as_str().unwrap_or("").to_lowercase();
                    let cui = val["canonical_cui"].as_str().unwrap_or("").to_string();
                    let synonyms: Vec<String> = val["synonyms"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s["name"].as_str().map(|n| n.to_lowercase()))
                                .collect()
                        })
                        .unwrap_or_default();
                    umls_map.insert(ingredient, (cui, synonyms));
                }
            }
            println!("  Loaded {} UMLS CUI entries", umls_map.len());
        } else {
            println!("  Warning: could not read {}", cuis_path);
        }

        // ── Load CTD chemicals ──────────────────────────────────────────
        let ctd_path = format!("{}/ctd/CTD_chemicals.csv", data_dir);
        let mut ctd_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new(); // lowercase name/synonym -> mesh_id

        if let Ok(content) = std::fs::read_to_string(&ctd_path) {
            for line in content.lines() {
                if line.starts_with('#') { continue; }
                // CSV format: ChemicalName,ChemicalID,...,MESHSynonyms,CTDCuratedSynonyms
                let parts: Vec<&str> = line.splitn(13, ',').collect();
                if parts.len() >= 2 {
                    let chem_name = parts[0].trim_matches('"').to_lowercase();
                    let mesh_id = parts[1].replace("MESH:", "");
                    ctd_map.insert(chem_name.clone(), mesh_id.clone());

                    // Also index synonyms (field 11 = MESHSynonyms, field 12 = CTDCuratedSynonyms)
                    if parts.len() >= 12 {
                        for syn in parts[11].split('|') {
                            let s = syn.trim().to_lowercase();
                            if !s.is_empty() {
                                ctd_map.entry(s).or_insert(mesh_id.clone());
                            }
                        }
                    }
                }
            }
            println!("  Loaded {} CTD chemical entries", ctd_map.len());
        } else {
            println!("  Warning: could not read {} (run may still succeed)", ctd_path);
        }

        // ── Curated search terms for known ingredients ──────────────────
        // These are terms we know work well for SuppKG sentence search,
        // based on our analysis of the 19 test ingredients.
        let curated_search_terms: std::collections::HashMap<&str, Vec<&str>> = [
            ("magnesium", vec!["magnesium"]),
            ("quercetin", vec!["quercetin"]),
            ("zinc", vec!["zinc"]),
            ("vitamin d", vec!["vitamin d", "cholecalciferol", "calciferol"]),
            ("vitamin c", vec!["vitamin c", "ascorbic acid"]),
            ("ashwagandha", vec!["ashwagandha", "withania"]),
            ("probiotics", vec!["probiotic", "lactobacill", "bifidobact"]),
            ("coq10", vec!["coq10", "coenzyme q", "ubiquinone", "ubiquinol"]),
            ("nac", vec!["n-acetylcysteine", "n-acetyl-l-cysteine", "n-acetyl cysteine"]),
            ("rhodiola rosea", vec!["rhodiola"]),
            ("vitamin b complex", vec!["vitamin b", "b-vitamin", "b vitamin"]),
            ("alpha-lipoic acid", vec!["alpha-lipoic", "lipoic acid"]),
            ("melatonin", vec!["melatonin"]),
            ("fish oil", vec!["fish oil", "omega-3", "omega 3"]),
            ("turmeric", vec!["turmeric", "curcumin"]),
            ("iron", vec!["iron"]),
            ("calcium", vec!["calcium"]),
            ("gaba", vec!["gamma-aminobutyric", "gaba"]),
            ("theanine", vec!["theanine", "l-theanine"]),
        ].iter().cloned().collect();

        // ── Build registry records ──────────────────────────────────────
        let mut upserted = 0;
        for name in &ingredient_names {
            let umls_data = umls_map.get(name.as_str());
            let idisk_data = idisk_map.get(name.as_str());
            let ctd_mesh = ctd_map.get(name.as_str()).cloned().unwrap_or_default();

            // Build synonyms from all sources
            let mut synonyms: Vec<String> = Vec::new();
            if let Some((_, syns)) = umls_data {
                synonyms.extend(syns.iter().cloned());
            }
            if let Some((_, _, common_names, _)) = idisk_data {
                for cn in common_names {
                    if !synonyms.contains(cn) {
                        synonyms.push(cn.clone());
                    }
                }
            }
            // Deduplicate and remove the name itself
            synonyms.retain(|s| s != name && !s.is_empty());
            synonyms.sort();
            synonyms.dedup();

            // Search terms: use curated if available, else name + key synonyms
            let search_terms = if let Some(curated) = curated_search_terms.get(name.as_str()) {
                curated.iter().map(|s| s.to_string()).collect()
            } else {
                // For uncurated ingredients, use name as search term
                vec![name.clone()]
            };

            let record = IngredientRecord {
                name: name.clone(),
                synonyms,
                search_terms,
                umls_cui: umls_data.map(|(cui, _)| cui.clone()).unwrap_or_default(),
                idisk_id: idisk_data.map(|(id, _, _, _)| id.clone()).unwrap_or_default(),
                idisk_cui: idisk_data.map(|(_, cui, _, _)| cui.clone()).unwrap_or_default(),
                ctd_mesh,
                suppkg_cui: String::new(), // Populated by CUI resolution, not hydration
            };

            registry.upsert(&record).await;
            upserted += 1;
        }

        let after = registry.count().await;
        println!();
        println!("─── Ingredient Registry ────────────────────────────────");
        println!("  {} ingredients hydrated ({} existed before)", upserted, before);
        println!("  {} total in registry", after);

        // Print summary
        let all = registry.list_all().await;
        for rec in &all {
            let sources: Vec<&str> = [
                if !rec.umls_cui.is_empty() { Some("UMLS") } else { None },
                if !rec.idisk_id.is_empty() { Some("iDISK") } else { None },
                if !rec.ctd_mesh.is_empty() { Some("CTD") } else { None },
            ].into_iter().flatten().collect();
            println!(
                "    {:<25} search_terms: {:?}  sources: [{}]",
                rec.name,
                rec.search_terms,
                sources.join(", ")
            );
        }
        println!();

        return;
    }

    // --cite-only: run citation backing against existing graph and exit
    if cli.cite_only {
        // Resolve SuppKG path: --suppkg flag → SUPPKG_PATH env → error
        let suppkg_dir = cli.suppkg.clone()
            .or_else(|| {
                std::env::var("SUPPKG_PATH").ok().map(|p| {
                    // SUPPKG_PATH may point to the json file; use its parent dir
                    std::path::Path::new(&p)
                        .parent()
                        .map(|d| d.to_string_lossy().to_string())
                        .unwrap_or(p)
                })
            });

        let dir = match suppkg_dir {
            Some(d) => d,
            None => {
                eprintln!("--cite-only requires SuppKG. Pass --suppkg <dir> or set SUPPKG_PATH.");
                std::process::exit(1);
            }
        };

        let json_path = format!("{}/supp_kg.json", dir);
        let edgelist_path = format!("{}/suppkg_v2.edgelist", dir);

        print!("  Loading SuppKG from {}/ ...", dir);
        let kg_result = if std::path::Path::new(&edgelist_path).exists() {
            SuppKg::load_with_edgelist(&json_path, &edgelist_path)
        } else {
            SuppKg::load(&json_path)
        };

        let kg = match kg_result {
            Ok(kg) => {
                println!(
                    " {} nodes, {} terms, {} edge pairs",
                    kg.node_count(), kg.term_count(), kg.edge_pair_count()
                );
                kg
            }
            Err(e) => {
                eprintln!(" Error loading SuppKG: {}", e);
                std::process::exit(1);
            }
        };

        let sink = match JsonlFileSink::new(&cli.output) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error creating event log: {}", e);
                std::process::exit(1);
            }
        };

        println!("  Running citation backing ...");
        let cite_corr = Uuid::new_v4();
        let cite_result = nsai_loop::citations::run_citation_backing(
            &graph, &kg, &merge_store, &source_store, &sink, cite_corr,
        )
        .await;

        println!("─── Citation Backing ───────────────────────────────────");
        println!(
            "  {} edges checked, {} backed by PubMed, {} citations stored",
            cite_result.edges_checked, cite_result.edges_backed, cite_result.citations_stored
        );
        println!(
            "  Resolution: {} via CUI, {} via sentence search",
            cite_result.cui_resolved, cite_result.sentence_resolved
        );
        println!();

        sink.flush().expect("Failed to flush event log");
        return;
    }

    // Load SuppKG: --suppkg flag → SUPPKG_PATH env var → skip
    let suppkg_resolved_dir = cli.suppkg.clone().or_else(|| {
        std::env::var("SUPPKG_PATH").ok().map(|p| {
            std::path::Path::new(&p)
                .parent()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or(p)
        })
    });
    let suppkg_data = if let Some(ref dir) = suppkg_resolved_dir {
        let json_path = format!("{}/supp_kg.json", dir);
        let edgelist_path = format!("{}/suppkg_v2.edgelist", dir);

        print!("  Loading SuppKG from {}/ ...", dir);
        let result = if std::path::Path::new(&edgelist_path).exists() {
            SuppKg::load_with_edgelist(&json_path, &edgelist_path)
        } else {
            SuppKg::load(&json_path)
        };
        match result {
            Ok(kg) => {
                println!(
                    " {} nodes, {} terms, {} edge pairs",
                    kg.node_count(),
                    kg.term_count(),
                    kg.edge_pair_count()
                );
                println!();
                Some(kg)
            }
            Err(e) => {
                eprintln!(" Error: {}", e);
                None
            }
        }
    } else {
        None
    };

    // Run the NSAI loop for each nutraceutical into the shared graph
    let config = nsai_loop::loop_runner::LoopConfig {
        max_gap_iterations: cli.max_iterations,
        max_gaps_per_iteration: cli.max_gaps,
        max_speculative_observations: cli.max_speculative,
        ..Default::default()
    };

    let mut nsai = NsaiLoop::new(provider.as_ref(), &sink)
        .with_config(config)
        .with_lens(ingestion_lens)
        .with_source_store(&source_store);

    if let Some(ref kg) = suppkg_data {
        nsai = nsai.with_synonym_resolution(kg, &merge_store);
        // Wire up UMLS API if key is available
        if let Ok(umls_key) = std::env::var("UMLS_API_KEY") {
            let cache_path = std::path::Path::new(&cli.output)
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("supplement_cuis.jsonl");
            nsai = nsai.with_umls(umls_key, cache_path);
        }
    }

    for nutra in &nutraceuticals {
        let correlation_id = Uuid::new_v4();
        println!("─── {} ─────────────────────────────────────────────", nutra);
        println!("  Correlation ID: {}", correlation_id);
        println!();

        let result = nsai.run(nutra, &graph, correlation_id).await;

        println!("  Seed + {} gap-fill iterations", result.iterations);
        println!("  Gaps addressed: {}", result.total_gaps_filled);
        if result.synonym_cuis_assigned > 0 || result.synonym_aliases_found > 0 {
            println!(
                "  Synonyms:       {} CUIs assigned, {} aliases found",
                result.synonym_cuis_assigned, result.synonym_aliases_found
            );
        }
        if result.deduced_chains > 0 {
            println!(
                "  Deduced:        {} chains, {} edges added",
                result.deduced_chains, result.deduced_edges_added
            );
        }
        println!(
            "  Comprehension:  {} confirmed, {} new",
            result.comprehension_edges_confirmed, result.comprehension_edges_new
        );
        if result.speculative_observations > 0 {
            println!(
                "  Speculative:    {} observations, {} edges added",
                result.speculative_observations, result.speculative_edges_added
            );
        }
        if result.citations_stored > 0 {
            println!(
                "  Citations:      {} stored from SuppKG",
                result.citations_stored
            );
        }
        println!(
            "  Graph now:      {} nodes, {} edges",
            result.final_node_count, result.final_edge_count
        );
        println!();
    }

    // Synonym resolution summary
    let alias_count = merge_store.alias_count().await;
    let cui_count = merge_store.cui_count().await;
    if alias_count > 0 || cui_count > 0 {
        println!("─── Synonym Resolution ─────────────────────────────────");
        println!("  {} CUI mappings, {} aliases", cui_count, alias_count);
        for alias in merge_store.all_aliases().await {
            println!(
                "    {} = {} (conf: {:.2}, method: {})",
                alias.alias, alias.canonical, alias.confidence, alias.method
            );
        }
        println!();
    }

    // Citation backing — match graph edges to SuppKG PubMed citations
    if let Some(ref kg) = suppkg_data {
        let cite_corr = Uuid::new_v4();
        let cite_result = nsai_loop::citations::run_citation_backing(
            &graph, kg, &merge_store, &source_store, &sink, cite_corr,
        )
        .await;
        if cite_result.citations_stored > 0 || cite_result.edges_backed > 0 {
            println!("─── Citation Backing ───────────────────────────────────");
            println!(
                "  {} edges checked, {} backed by PubMed, {} citations stored",
                cite_result.edges_checked, cite_result.edges_backed, cite_result.citations_stored
            );
            println!(
                "  Resolution: {} via CUI, {} via sentence search",
                cite_result.cui_resolved, cite_result.sentence_resolved
            );
            println!();
        }
    }

    // Cross-provider confidence boosting
    let boost_result = nsai_loop::confidence::boost_multi_provider_confidence(&graph, &source_store).await;
    if boost_result.edges_boosted > 0 {
        println!("─── Cross-Provider Confidence ──────────────────────────");
        println!(
            "  {} edges observed by multiple providers, {} boosted (+0.15)",
            boost_result.multi_provider_edges, boost_result.edges_boosted
        );
        println!();
    }

    // Confidence decay — unconfirmed speculative/deduced edges lose confidence
    let decay_result = nsai_loop::confidence::decay_unconfirmed_confidence(&graph, &source_store).await;
    if decay_result.decayed > 0 {
        println!("─── Confidence Decay ───────────────────────────────────");
        println!(
            "  {} speculative/deduced edges eligible, {} decayed (-0.05)",
            decay_result.eligible, decay_result.decayed
        );
        println!();
    }

    // Structural inference — find cross-ingredient patterns
    let all_ingredients = graph.nodes_by_type(&graph_service::types::NodeType::Ingredient).await;
    if all_ingredients.len() >= 2 {
        let observations = nsai_loop::structural::find_observations(&graph).await;
        if !observations.is_empty() {
            println!("─── Structural Observations ────────────────────────────");
            println!("  {} patterns found across {} ingredients\n", observations.len(), all_ingredients.len());
            for (i, obs) in observations.iter().enumerate() {
                println!("  {}. [{:?}] {} (score: {:.1})", i + 1, obs.kind, obs.description, obs.score);
            }
            println!();
        }
    }

    // Coverage check — structural completeness per ingredient
    let coverage = nsai_loop::analyzer::coverage_check(&graph).await;
    let incomplete: Vec<_> = coverage.iter().filter(|c| !c.is_complete()).collect();
    if !incomplete.is_empty() {
        println!("─── Coverage Gaps ──────────────────────────────────────");
        for report in &incomplete {
            println!("  {} is missing:", report.ingredient);
            for m in report.missing() {
                println!("    - {}", m);
            }
        }
        println!();
    }

    // Print the final combined graph
    println!("─── Knowledge Graph ────────────────────────────────────");
    println!("  Nodes: {}", graph.node_count().await);
    println!("  Edges: {}", graph.edge_count().await);
    println!();
    println!("{}", graph.dump().await);

    sink.flush().expect("Failed to flush event log");

    println!("  Events written to {}", cli.output);
    println!(
        "  View log: cargo run --bin log-viewer -- {}",
        cli.output
    );
}
