use clap::Parser;
use event_log::sink::{EventSink, JsonlFileSink};
use graph_service::graph::KnowledgeGraph;
use graph_service::source::SourceStore;
use llm_client::anthropic::AnthropicProvider;
use llm_client::gemini::GeminiProvider;
use llm_client::mock::MockProvider;
use llm_client::provider::LlmProvider;
use nsai_loop::loop_runner::NsaiLoop;
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
            "Unknown provider: {}. Use: anthropic, gemini, mock",
            other
        )),
    }
}

#[tokio::main]
async fn main() {
    load_env();
    let cli = Cli::parse();

    let nutraceuticals: Vec<String> = cli.nutraceutical.iter().map(|s| s.trim().to_string()).collect();

    let db_path = cli.graph_db.clone().unwrap_or_else(default_db_path);

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║      supplementbot — NSAI loop (5th grade)          ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();
    println!("  Nutraceuticals: {}", nutraceuticals.join(", "));
    println!("  Provider:       {}", cli.provider);
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

    // Run the NSAI loop for each nutraceutical into the shared graph
    let config = nsai_loop::loop_runner::LoopConfig {
        max_gap_iterations: cli.max_iterations,
        max_gaps_per_iteration: cli.max_gaps,
        max_speculative_observations: cli.max_speculative,
    };

    let nsai = NsaiLoop::new(provider.as_ref(), &sink)
        .with_config(config)
        .with_source_store(&source_store);

    for nutra in &nutraceuticals {
        let correlation_id = Uuid::new_v4();
        println!("─── {} ─────────────────────────────────────────────", nutra);
        println!("  Correlation ID: {}", correlation_id);
        println!();

        let result = nsai.run(nutra, &graph, correlation_id).await;

        println!("  Seed + {} gap-fill iterations", result.iterations);
        println!("  Gaps addressed: {}", result.total_gaps_filled);
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

    // Structural inference — find cross-ingredient patterns
    let all_ingredients = graph.nodes_by_type(&graph_service::types::NodeType::Ingredient).await;
    if all_ingredients.len() >= 2 {
        let observations = nsai_loop::structural::find_observations(&graph).await;
        if !observations.is_empty() {
            println!("─── Structural Observations ────────────────────────────");
            println!("  {} patterns found across {} ingredients\n", observations.len(), all_ingredients.len());
            for (i, obs) in observations.iter().enumerate() {
                println!("  {}. [{:?}] {}", i + 1, obs.kind, obs.description);
            }
            println!();
        }
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
