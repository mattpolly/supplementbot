# Supplementbot

A neurosymbolic AI system for systemic wellness that maps how nutraceuticals interact with the human body. It combines LLM-based extraction (the neural layer) with a typed knowledge graph (the symbolic layer) to build structured, auditable representations of supplement science — then uses that knowledge to guide real conversations with users about their symptoms.

**Legal constraint:** This system never diagnoses and never uses the word "cure." All language is framed around symptoms and body systems.

## Architecture

Supplementbot has two main layers:

1. **Knowledge building** — An NSAI loop teaches a knowledge graph about supplements by iteratively querying LLMs, extracting typed triples, analyzing gaps, and running structural inference. The graph persists in SurrealDB and grows across runs.

2. **Intake conversation** — A web-facing agent conducts a clinical-style interview over WebSocket. An **intake knowledge graph** (process knowledge) drives the conversation — selecting questions via Expected Information Gain scoring, while a **supplement knowledge graph** (domain knowledge) provides the facts. The agent narrows from symptoms to supplement candidates across six phases.

```
User ↔ WebSocket ↔ Web Server
                      ├── Safety filter (red flags, post-gen)
                      ├── Extraction (cheap LLM → structured data)
                      ├── Concept mapping (text → graph nodes)
                      ├── Intake KG engine (next question + graph actions)
                      ├── Graph executor (supplement KG + iDISK queries)
                      ├── Context builder (LLM prompt assembly)
                      └── Renderer LLM (natural language response)
```

### Intake Phases

| Phase | Purpose |
|-------|---------|
| Chief Complaint | Capture what brought the user here |
| HPI (OLDCARTS) | Characterize symptoms: onset, location, duration, character, aggravating/alleviating, radiation, timing, severity |
| Review of Systems | Screen adjacent body systems |
| Differentiation | Discriminate between top supplement candidates |
| Causation Inquiry | Check if symptoms may be adverse reactions to current supplements |
| Recommendation | Deliver ranked supplements with evidence and safety caveats |

### Knowledge Graph

The supplement KG uses 14 node types across three complexity tiers (foundational → intermediate → advanced) and 19 edge predicates. A **complexity lens** (continuous 0.0–1.0 dial) gates which types are visible, preventing advanced biochemistry from leaking into simple explanations.

External data sources:
- **iDISK 2.0** — 392 symptoms, 7,876 ingredients, 214 drugs, interaction/adverse reaction edges
- **SuppKG** — 570,000 literature-extracted edges linking dietary compounds to clinical concepts. Used for synonym resolution (CUI assignment) and citation backing. Edges are not imported into the graph directly; instead, SuppKG citations are stored in `edge_citation` and surfaced at recommendation time.

## Project Structure

```
supplementbot/
├── crates/
│   ├── cli/              # CLI for NSAI loop runs
│   ├── curriculum/       # Question generation by grade level
│   ├── event-log/        # Structured JSONL observability
│   ├── extraction/       # LLM response → typed graph triples
│   ├── graph-service/    # Supplement KG + intake KG + ontology + complexity lens
│   │   └── src/intake/   #   Intake KG engine, executor, iDISK importer, seed data
│   ├── intake-agent/     # Session state, phase logic, safety, concept mapping, context builder
│   ├── llm-client/       # Provider-agnostic LLM trait (Anthropic, Gemini, xAI/Grok, mock)
│   ├── log-viewer/       # Terminal viewer for event logs
│   ├── nsai-loop/        # Loop orchestrator (gap analysis, filling, comprehension, structural inference)
│   ├── suppkg/           # SuppKG data loader (lookup-only)
│   └── web-server/       # Axum HTTP + WebSocket server, turn pipeline, session management
├── docs/                 # Architecture docs (TECHNICAL.md, INTAKE.md, etc.)
├── data/                 # iDISK + SuppKG data files (gitignored, ~683 MB)
└── Cargo.toml            # Workspace root
```

## Quick Start

### Prerequisites

- Rust (stable toolchain)
- `libclang-dev` (for SurrealDB's RocksDB backend: `sudo apt install libclang-dev`)
- API keys for LLM providers

### Build

```bash
cargo build --release
```

First build takes several minutes due to SurrealDB compilation. Subsequent builds are fast.

### Run Tests

```bash
cargo test
```

### Run the Web Server

```bash
# Required: at least one LLM provider key
export ANTHROPIC_API_KEY="..."    # renderer LLM
export XAI_API_KEY="..."          # extractor LLM (or use Anthropic for both)

# Optional
export IDISK_DATA_DIR="./data"    # path to iDISK data files
export GRAPH_PATH="~/.supplementbot/graph"
export STATIC_DIR="./static"
export PORT=3000

cargo run --bin supplementbot-web
```

Connect via WebSocket at `ws://localhost:3000/ws/chat`. Health check at `GET /api/health`.

#### Web Server Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `HOST` | `0.0.0.0` | Bind address |
| `PORT` | `3000` | Bind port |
| `GRAPH_PATH` | `~/.supplementbot/graph` | SurrealDB database path |
| `STATIC_DIR` | — | Static file directory for frontend |
| `IDISK_DATA_DIR` | — | iDISK 2.0 data directory |
| `MAX_CONCURRENT_SESSIONS` | `2` | Max simultaneous sessions |
| `DAILY_SESSION_CAP` | `10` | Daily session limit |
| `MONTHLY_SESSION_CAP` | `100` | Monthly session limit |
| `SESSION_TIMEOUT_SECS` | `900` | Idle session timeout (15 min) |

### Run the CLI (NSAI Loop)

```bash
# With the mock provider (no API key needed)
cargo run --bin supplementbot -- --nutraceutical "Magnesium"

# With Anthropic Claude
cargo run --bin supplementbot -- --nutraceutical "Magnesium" --provider anthropic

# With Google Gemini
cargo run --bin supplementbot -- --nutraceutical "Magnesium" --provider gemini

# Multiple nutraceuticals in one run
cargo run --bin supplementbot -- --nutraceutical "Magnesium,Zinc" --provider anthropic

# Graph persists between runs
cargo run --bin supplementbot -- -n Magnesium -p anthropic
cargo run --bin supplementbot -- -n Zinc -p anthropic
```

#### CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `-n, --nutraceutical` | — | Supplement(s) to analyze (comma-separated) |
| `-p, --provider` | `mock` | LLM provider: `anthropic`, `gemini`, or `mock` |
| `-m, --model` | provider default | Model name override |
| `-g, --graph-db` | `~/.supplementbot/graph` | Path to persistent graph database |
| `-o, --output` | `events.jsonl` | Event log output file |
| `--max-iterations` | `3` | Max gap-filling iterations |
| `--max-gaps` | `5` | Max gaps to fill per iteration |
| `--suppkg` | — | Path to SuppKG data directory (required for `--resolve-cuis` and `--cite-only`) |
| `--resolve-cuis` | — | Populate merge store CUIs from SuppKG for all graph nodes, then exit. Run this before `--cite-only` if the graph was built without `--suppkg`. |
| `--cite-only` | — | Run citation backing against the existing graph using SuppKG, then exit. No LLM calls. Stores citations in `edge_citation` table. |

#### Retroactive citation backing

After building the graph with LLM runs, populate PubMed-sourced citations without re-running the LLMs:

```bash
# Step 1: assign CUIs to graph nodes (only needed once, or if graph was rebuilt)
./target/release/supplementbot --graph-db ./data/graph --suppkg ./data/suppkg --resolve-cuis

# Step 2: store citations from SuppKG into edge_citation table
./target/release/supplementbot --graph-db ./data/graph --suppkg ./data/suppkg --cite-only
```

Citation backing is ingredient-level: for each `Ingredient` node, the system finds its best CUI, retrieves all outgoing SuppKG edges, and stores supporting sentences. Hardcoded CUI overrides correct known mismatches in the SuppKG term index (e.g. "magnesium" → magnesium stearate, not dietary magnesium).

### View Event Logs

```bash
cargo run --bin log-viewer -- events.jsonl
cargo run --bin log-viewer -- events.jsonl --filter extraction
cargo run --bin log-viewer -- events.jsonl --filter gap
```

## Documentation

- [TECHNICAL.md](docs/TECHNICAL.md) — Full architecture deep dive, crate by crate
- [INTAKE.md](docs/INTAKE.md) — Intake KG + agent design, walkthrough, and decisions

## License

Private — not yet licensed for distribution.
