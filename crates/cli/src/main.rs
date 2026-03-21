use clap::Parser;
use curriculum::agent::CurriculumAgent;
use event_log::sink::{EventSink, JsonlFileSink};
use llm_client::anthropic::AnthropicProvider;
use llm_client::gemini::GeminiProvider;
use llm_client::mock::MockProvider;
use llm_client::provider::LlmProvider;
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "supplementbot")]
#[command(about = "Neurosymbolic AI for systemic wellness — curriculum runner")]
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
}

fn build_provider(cli: &Cli) -> Result<Box<dyn LlmProvider>, String> {
    match cli.provider.as_str() {
        "anthropic" => {
            let model = cli
                .model
                .clone()
                .unwrap_or_else(|| "claude-sonnet-4-20250514".into());
            AnthropicProvider::from_env(model)
                .map(|p| Box::new(p) as Box<dyn LlmProvider>)
                .map_err(|e| e.to_string())
        }
        "gemini" => {
            let model = cli
                .model
                .clone()
                .unwrap_or_else(|| "gemini-2.0-flash".into());
            GeminiProvider::from_env(model)
                .map(|p| Box::new(p) as Box<dyn LlmProvider>)
                .map_err(|e| e.to_string())
        }
        "mock" => {
            let provider = MockProvider::new("mock", "mock-v1")
                .on(
                    "physiological systems",
                    "Magnesium acts on the following physiological systems:\n\n\
                     1. **Nervous System** — Magnesium blocks NMDA receptors, reducing excitatory \
                     neurotransmission. It also acts as a positive allosteric modulator of GABA-A \
                     receptors, enhancing inhibitory signaling. Involved in HPA axis regulation \
                     affecting cortisol release.\n\n\
                     2. **Gastrointestinal System** — Promotes smooth muscle relaxation in the GI \
                     tract, modulating motility. Osmotic effects in the intestinal lumen support \
                     bowel regularity.\n\n\
                     3. **Musculoskeletal System** — Regulates muscle contraction and relaxation by \
                     competing with calcium at voltage-gated calcium channels. Essential cofactor \
                     for ATP-dependent muscle fiber function.\n\n\
                     4. **Immune System** — Modulates NF-κB signaling pathway, influencing \
                     pro-inflammatory cytokine production (IL-6, TNF-α). Supports antioxidant \
                     defense via glutathione synthesis.",
                )
                .on(
                    "mechanisms of action",
                    "Known mechanisms of action for Magnesium:\n\n\
                     1. **NMDA Receptor Antagonism** — Voltage-dependent block of the NMDA receptor \
                     ion channel by Mg²⁺ ions. At resting membrane potential, Mg²⁺ occupies the \
                     channel pore, preventing calcium influx. Relevant to excitotoxicity protection \
                     and pain signaling.\n\n\
                     2. **GABA-A Receptor Positive Allosteric Modulation** — Enhances chloride ion \
                     conductance through GABA-A receptors, promoting inhibitory neurotransmission.\n\n\
                     3. **Voltage-Gated Calcium Channel Regulation** — Competes with Ca²⁺ at L-type \
                     and T-type calcium channels, modulating smooth muscle tone, cardiac rhythm, and \
                     neurotransmitter release.\n\n\
                     4. **NF-κB Pathway Modulation** — Reduces nuclear translocation of NF-κB, \
                     downregulating transcription of pro-inflammatory cytokines.\n\n\
                     5. **ATP Cofactor Activity** — Mg²⁺-ATP complex is the biologically active form \
                     of ATP. Required for kinase activity, glycolysis, and oxidative phosphorylation.",
                )
                .on(
                    "therapeutic uses",
                    "Primary therapeutic uses of Magnesium supplementation:\n\n\
                     1. **Muscle cramping and tension** — Addresses involuntary muscle contraction \
                     through calcium channel regulation and ATP-dependent muscle fiber relaxation. \
                     Commonly reported in nocturnal leg cramps.\n\n\
                     2. **Sleep difficulty and restlessness** — Supports sleep onset via GABA-A \
                     receptor modulation (promoting inhibitory tone) and NMDA antagonism (reducing \
                     excitatory activity). Also modulates melatonin synthesis.\n\n\
                     3. **Stress-related symptoms** — Modulates HPA axis reactivity, influencing \
                     cortisol output. Bidirectional relationship: stress depletes magnesium, and \
                     low magnesium amplifies stress response.\n\n\
                     4. **Irregular bowel motility** — Osmotic effect in the intestinal lumen draws \
                     water into the bowel, and smooth muscle relaxation modulates peristalsis.\n\n\
                     5. **Inflammatory symptoms** — Via NF-κB pathway suppression, reduces \
                     pro-inflammatory cytokine production. Supports glutathione synthesis for \
                     oxidative stress management.",
                )
                .with_default("This is a mock response. Configure mock data for this prompt.");
            Ok(Box::new(provider))
        }
        other => Err(format!("Unknown provider: {}. Use: anthropic, gemini, mock", other)),
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    println!("╔══════════════════════════════════════════════════════╗");
    println!("║           supplementbot — curriculum runner          ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!();
    println!("  Nutraceutical:  {}", cli.nutraceutical);
    println!("  Provider:       {}", cli.provider);
    println!("  Event log:      {}", cli.output);
    println!();

    let provider = match build_provider(&cli) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    println!(
        "  Model:          {}",
        provider.model_name()
    );
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
    println!("─── Stage 1: Foundational ───────────────────────────");
    println!();

    let agent = CurriculumAgent::new(provider.as_ref(), &sink);
    let results = agent.run_stage1(&cli.nutraceutical, correlation_id).await;

    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(resp) => {
                println!(
                    "  [{}] {:?} — {} ms",
                    i + 1,
                    resp.question.question_type,
                    resp.latency_ms
                );
                println!("  ┌─────────────────────────────────────────────");
                for line in resp.raw_response.lines() {
                    println!("  │ {}", line);
                }
                println!("  └─────────────────────────────────────────────");
                println!();
            }
            Err(e) => {
                println!("  [{}] ERROR: {}", i + 1, e);
                println!();
            }
        }
    }

    sink.flush().expect("Failed to flush event log");

    let success_count = results.iter().filter(|r| r.is_ok()).count();
    println!("─── Done ───────────────────────────────────────────");
    println!(
        "  {}/{} questions answered. Events written to {}",
        success_count,
        results.len(),
        cli.output
    );
    println!(
        "  View log: cargo run --bin log-viewer -- {}",
        cli.output
    );
}
