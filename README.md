# Supplementbot

A neurosymbolic AI system for systemic wellness that maps how nutraceuticals interact with the human body. It combines LLM-based extraction (the neural layer) with a typed knowledge graph (the symbolic layer) to build structured, auditable representations of supplement science.

**Legal constraint:** This system never diagnoses and never uses the word "cure." All language is framed around symptoms and treatments.

## How It Works

Supplementbot uses an iterative **NSAI loop** to teach a knowledge graph about supplements:

1. **Seed** — Ask an LLM a simple question about a supplement ("What does magnesium do?")
2. **Extract** — Parse the response into typed graph triples (nodes + edges)
3. **Analyze gaps** — Inspect the graph for missing connections (leaf nodes, missing mechanisms)
4. **Fill gaps** — Ask targeted follow-up questions and extract the answers
5. **Comprehension check** — Have the LLM rephrase its understanding; re-extract and compare for consistency

The graph grows denser each iteration. A **complexity lens** (continuous 0.0–1.0 dial) controls which ontology types are visible at each grade level, preventing advanced biochemistry from leaking into simple explanations.

## Project Structure

```
supplementbot/
├── crates/
│   ├── cli/              # Command-line interface
│   ├── curriculum/       # Question generation by grade level
│   ├── event-log/        # Structured JSONL observability
│   ├── extraction/       # LLM response → typed graph triples
│   ├── graph-service/    # Knowledge graph (petgraph) + ontology types + complexity lens
│   ├── llm-client/       # Provider-agnostic LLM trait (Anthropic, Gemini, mock)
│   ├── log-viewer/       # Terminal viewer for event logs
│   └── nsai-loop/        # Loop orchestrator (gap analysis, filling, comprehension)
└── Cargo.toml            # Workspace root
```

## Quick Start

### Prerequisites

- Rust (stable toolchain)
- An API key for at least one LLM provider

### Build

```bash
cargo build --release
```

### Run Tests

```bash
cargo test
```

67 tests across 8 crates covering graph operations, extraction parsing, lens enforcement, gap analysis, comprehension checks, and the full NSAI loop.

### Run the CLI

```bash
# With the mock provider (no API key needed)
cargo run --bin cli -- --nutraceutical "Magnesium"

# With Anthropic Claude
ANTHROPIC_API_KEY=sk-... cargo run --bin cli --features anthropic -- --nutraceutical "Magnesium"

# With Google Gemini
GEMINI_API_KEY=... cargo run --bin cli --features gemini -- --nutraceutical "Magnesium"
```

#### CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--nutraceutical` | `"Magnesium"` | Supplement to analyze |
| `--max-iterations` | `3` | Max gap-filling iterations |
| `--max-gaps` | `5` | Max gaps to fill per iteration |

### View Event Logs

```bash
# View all events
cargo run --bin log-viewer -- events.jsonl

# Filter by type
cargo run --bin log-viewer -- events.jsonl --filter extraction
cargo run --bin log-viewer -- events.jsonl --filter gap
cargo run --bin log-viewer -- events.jsonl --filter comprehension
cargo run --bin log-viewer -- events.jsonl --filter loop
```

## Current Scope

The system currently operates at **5th grade level only** — proving the architecture works before adding complexity escalation. The ontology and lens system are designed for the full range (5th grade → graduate), but only the foundational tier is active.

### Starting Nutraceuticals (planned)

Magnesium, Zinc, Vitamin D, Omega-3 fatty acids, B-complex vitamins, Vitamin C, Curcumin, Probiotics, Ashwagandha, CoQ10

## What This Demonstrates

- **Neurosymbolic AI** — combining neural (LLM) and symbolic (graph) reasoning
- **Affordance-based modeling** — "magnesium affords muscle relaxation" rather than rigid lookups
- **Ontology complexity gating** — continuous dial prevents grade-inappropriate concepts from leaking
- **Self-consistency checking** — rephrase test validates understanding before escalating
- **Provider-agnostic design** — swap LLM providers without changing application logic
- **Full observability** — every LLM call, extraction, and graph mutation is logged with correlation IDs

## License

Private — not yet licensed for distribution.
