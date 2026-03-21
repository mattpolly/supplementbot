use chrono::{DateTime, Utc};
use clap::Parser;
use event_log::events::{EventEnvelope, PipelineEvent, ReviewVerdict};
use event_log::sink::JsonlFileSink;

#[derive(Parser)]
#[command(name = "log-viewer")]
#[command(about = "Pretty-print supplementbot event logs")]
struct Cli {
    /// Path to the JSONL event log file
    file: String,

    /// Filter by correlation ID (prefix match)
    #[arg(short, long)]
    correlation: Option<String>,

    /// Only show events of this type (request, response, error, extraction, claim, review, mutation)
    #[arg(short, long)]
    filter: Option<String>,
}

fn main() {
    let cli = Cli::parse();

    let events = match JsonlFileSink::read_all(&cli.file) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Error reading {}: {}", cli.file, e);
            std::process::exit(1);
        }
    };

    if events.is_empty() {
        println!("No events found in {}", cli.file);
        return;
    }

    // Find the earliest timestamp for relative timing
    let base_time = events
        .iter()
        .map(|e| e.timestamp)
        .min()
        .unwrap();

    println!();
    println!("╔══════════════════════════════════════════════════════╗");
    println!("║              supplementbot — event log               ║");
    println!("╚══════════════════════════════════════════════════════╝");
    println!("  File: {}", cli.file);
    println!("  Events: {}", events.len());
    println!();

    for envelope in &events {
        // Filter by correlation ID prefix
        if let Some(ref prefix) = cli.correlation {
            if !envelope.correlation_id.to_string().starts_with(prefix) {
                continue;
            }
        }

        // Filter by event type
        if let Some(ref filter) = cli.filter {
            if !matches_filter(&envelope.event, filter) {
                continue;
            }
        }

        print_event(envelope, base_time);
    }
}

fn matches_filter(event: &PipelineEvent, filter: &str) -> bool {
    match filter {
        "request" => matches!(event, PipelineEvent::LlmRequest { .. }),
        "response" => matches!(event, PipelineEvent::LlmResponse { .. }),
        "error" => matches!(event, PipelineEvent::LlmError { .. }),
        "extraction" => matches!(
            event,
            PipelineEvent::ExtractionInput { .. } | PipelineEvent::ExtractionOutput { .. }
        ),
        "claim" => matches!(event, PipelineEvent::SpeculativeClaim { .. }),
        "review" => matches!(event, PipelineEvent::ReviewResult { .. }),
        "mutation" => matches!(
            event,
            PipelineEvent::GraphNodeMutation { .. } | PipelineEvent::GraphEdgeMutation { .. }
        ),
        _ => true,
    }
}

fn print_event(envelope: &EventEnvelope, base_time: DateTime<Utc>) {
    let elapsed = envelope.timestamp.signed_duration_since(base_time);
    let secs = elapsed.num_milliseconds() as f64 / 1000.0;
    let corr_short = &envelope.correlation_id.to_string()[..8];

    match &envelope.event {
        PipelineEvent::LlmRequest {
            provider,
            model,
            prompt,
            nutraceutical,
            stage,
            question_type,
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[34mREQUEST\x1b[0m ── {}/{} ({})",
                secs, provider, model, corr_short
            );
            println!(
                "              {} | {:?} | {}",
                nutraceutical, stage, question_type
            );
            let truncated = truncate(prompt, 120);
            println!("              \"{}\"", truncated);
            println!();
        }

        PipelineEvent::LlmResponse {
            provider,
            model,
            raw_response,
            latency_ms,
            tokens_used,
        } => {
            let token_str = tokens_used
                .as_ref()
                .map(|t| format!("{}→{} tokens", t.input_tokens, t.output_tokens))
                .unwrap_or_else(|| "tokens: n/a".into());

            println!(
                "  [{:>8.3}s] ── \x1b[32mRESPONSE\x1b[0m ── {}/{} ({}ms, {})",
                secs, provider, model, latency_ms, token_str
            );

            // Show first few lines of response
            for (i, line) in raw_response.lines().enumerate() {
                if i >= 8 {
                    println!("              ... ({} more lines)", raw_response.lines().count() - 8);
                    break;
                }
                println!("              │ {}", line);
            }
            println!();
        }

        PipelineEvent::LlmError {
            provider,
            model,
            error,
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[31mERROR\x1b[0m ── {}/{} ({})",
                secs, provider, model, corr_short
            );
            println!("              {}", error);
            println!();
        }

        PipelineEvent::ExtractionInput {
            nutraceutical,
            stage,
            ..
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[33mEXTRACTION IN\x1b[0m ── {} | {:?} ({})",
                secs, nutraceutical, stage, corr_short
            );
            println!();
        }

        PipelineEvent::ExtractionOutput {
            nodes_added,
            edges_added,
            parse_warnings,
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[33mEXTRACTION OUT\x1b[0m ── +{} nodes, +{} edges ({})",
                secs,
                nodes_added.len(),
                edges_added.len(),
                corr_short
            );
            for node in nodes_added {
                println!("              + {} ({})", node.name, node.node_type);
            }
            for edge in edges_added {
                println!(
                    "              + {} ──[{}]──▶ {} (conf: {:.2})",
                    edge.source, edge.edge_type, edge.target, edge.confidence
                );
            }
            for warning in parse_warnings {
                println!("              ⚠ {}", warning);
            }
            println!();
        }

        PipelineEvent::SpeculativeClaim {
            claim,
            topology_justification,
            source_nodes,
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[35mSPECULATIVE CLAIM\x1b[0m ── ({})",
                secs, corr_short
            );
            println!("              Claim: {}", claim);
            println!("              Justification: {}", topology_justification);
            println!("              Source nodes: {}", source_nodes.join(", "));
            println!();
        }

        PipelineEvent::ReviewResult {
            claim,
            provider_scores,
            final_confidence,
            verdict,
        } => {
            let verdict_str = match verdict {
                ReviewVerdict::Confirmed => "\x1b[32mCONFIRMED\x1b[0m",
                ReviewVerdict::Plausible => "\x1b[33mPLAUSIBLE\x1b[0m",
                ReviewVerdict::Contested => "\x1b[31mCONTESTED\x1b[0m",
                ReviewVerdict::Rejected => "\x1b[31mREJECTED\x1b[0m",
            };
            println!(
                "  [{:>8.3}s] ── \x1b[36mREVIEW\x1b[0m ── {} (conf: {:.2}) ({})",
                secs, verdict_str, final_confidence, corr_short
            );
            println!("              Claim: {}", claim);
            for score in provider_scores {
                println!(
                    "              {} → {:.2}",
                    score.provider, score.confidence
                );
            }
            println!();
        }

        PipelineEvent::GraphNodeMutation {
            operation,
            node_name,
            node_type,
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[36mGRAPH\x1b[0m ── {:?} node: {} ({}) ({})",
                secs, operation, node_name, node_type, corr_short
            );
        }

        PipelineEvent::GraphEdgeMutation {
            operation,
            source_node,
            target_node,
            edge_type,
            confidence,
        } => {
            println!(
                "  [{:>8.3}s] ── \x1b[36mGRAPH\x1b[0m ── {:?} edge: {} ──[{}]──▶ {} (conf: {:.2}) ({})",
                secs, operation, source_node, edge_type, target_node, confidence, corr_short
            );
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    // Take first line only, then truncate
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() > max {
        format!("{}...", &first_line[..max])
    } else {
        first_line.to_string()
    }
}
