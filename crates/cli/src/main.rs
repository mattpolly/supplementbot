use clap::Parser;
use event_log::sink::{EventSink, JsonlFileSink};
use graph_service::graph::KnowledgeGraph;
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
    /// The nutraceutical to study
    #[arg(short, long)]
    nutraceutical: String,

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

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║      supplementbot — NSAI loop (5th grade)          ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();
    println!("  Nutraceutical:  {}", cli.nutraceutical);
    println!("  Provider:       {}", cli.provider);
    println!("  Event log:      {}", cli.output);
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

    let correlation_id = Uuid::new_v4();
    println!("  Correlation ID: {}", correlation_id);
    println!();

    // Run the NSAI loop
    let config = nsai_loop::loop_runner::LoopConfig {
        max_gap_iterations: cli.max_iterations,
        max_gaps_per_iteration: cli.max_gaps,
    };

    let nsai = NsaiLoop::new(provider.as_ref(), &sink).with_config(config);
    let mut graph = KnowledgeGraph::new();

    println!("─── NSAI Loop ──────────────────────────────────────────");
    println!();
    println!("  Phase 1: Seed question (5th grade)...");

    let result = nsai.run(&cli.nutraceutical, &mut graph, correlation_id).await;

    println!("  Phase 2: Gap-filling ({} iterations)...", result.iterations);
    println!("    Gaps addressed: {}", result.total_gaps_filled);
    println!();
    println!("  Phase 3: Comprehension check...");
    println!(
        "    Edges confirmed: {}",
        result.comprehension_edges_confirmed
    );
    println!("    Edges new:       {}", result.comprehension_edges_new);
    println!();

    // Print the final graph
    println!("─── Knowledge Graph ────────────────────────────────────");
    println!("  Nodes: {}", result.final_node_count);
    println!("  Edges: {}", result.final_edge_count);
    println!();
    println!("{}", graph);

    sink.flush().expect("Failed to flush event log");

    println!("  Events written to {}", cli.output);
    println!(
        "  View log: cargo run --bin log-viewer -- {}",
        cli.output
    );
}
