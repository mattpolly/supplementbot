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

    /// Path to the graph database directory (default: ~/.supplementbot/graph)
    #[arg(short, long)]
    graph_db: Option<String>,

    /// Export the graph as JSON + HTML for visualization and exit
    #[arg(long, value_name = "PATH")]
    export: Option<String>,

    /// Enable SuppKG synonym resolution (default: data/ dir, or specify a custom dir)
    #[arg(long, value_name = "DIR", default_missing_value = "data", num_args = 0..=1)]
    suppkg: Option<String>,

    /// Complexity lens for extraction: 5th, 10th, college, graduate, or 0.0-1.0
    #[arg(short, long, default_value = "5th")]
    lens: String,
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

        /// Path to the graph database directory (default: ~/.supplementbot/graph)
        #[arg(short, long)]
        graph_db: Option<String>,
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
}

fn default_db_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/.supplementbot/graph", home)
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

async fn run_query(
    query_type: QueryType,
    lens_str: String,
    quality_str: Option<String>,
    min_confidence: Option<f64>,
    db_path: String,
) {
    let graph = match KnowledgeGraph::open(&db_path).await {
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

    // Handle query subcommand
    if let Some(Commands::Query {
        query_type,
        lens,
        quality,
        confidence,
        graph_db,
    }) = cli.command
    {
        let db_path = graph_db.unwrap_or_else(default_db_path);
        run_query(query_type, lens, quality, confidence, db_path).await;
        return;
    }

    let nutraceuticals: Vec<String> = cli.nutraceutical.iter().map(|s| s.trim().to_string()).collect();

    let db_path = cli.graph_db.clone().unwrap_or_else(default_db_path);

    let ingestion_lens = parse_lens(&cli.lens);

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║      supplementbot — NSAI loop                      ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();
    println!("  Nutraceuticals: {}", nutraceuticals.join(", "));
    println!("  Provider:       {}", cli.provider);
    println!("  Lens:           {} ({})", cli.lens, ingestion_lens.level());
    println!("  Event log:      {}", cli.output);
    println!("  Graph DB:       {}", db_path);
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

    // Open persistent graph database
    let graph = match KnowledgeGraph::open(&db_path).await {
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

    // Load SuppKG if --suppkg flag is present
    let suppkg_data = if let Some(ref dir) = cli.suppkg {
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
        if cite_result.citations_stored > 0 {
            println!("─── Citation Backing ───────────────────────────────────");
            println!(
                "  {} edges checked, {} backed by PubMed, {} citations stored",
                cite_result.edges_checked, cite_result.edges_backed, cite_result.citations_stored
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
