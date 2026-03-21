# Technical Architecture

Deep dive into how Supplementbot works, crate by crate.

---

## Table of Contents

- [Ontology](#ontology)
- [Complexity Lens](#complexity-lens)
- [Knowledge Graph](#knowledge-graph)
- [LLM Client](#llm-client)
- [Extraction Pipeline](#extraction-pipeline)
- [NSAI Loop](#nsai-loop)
- [Event System](#event-system)
- [Curriculum Agent](#curriculum-agent)

---

## Ontology

**Crate:** `graph-service` — `crates/graph-service/src/types.rs`

The ontology defines the vocabulary the system can reason about. Every node and edge has a type, and every type has a minimum complexity threshold.

### Node Types (7)

| Type | Min Complexity | Description |
|------|---------------|-------------|
| `Ingredient` | 0.0 | The supplement itself |
| `System` | 0.0 | A body system (nervous, muscular, etc.) |
| `Mechanism` | 0.1 | A biological process or pathway |
| `Property` | 0.0 | A therapeutic effect (muscle relaxation, sleep quality) |
| `Symptom` | 0.1 | A physiological sign (cramps, fatigue) |
| `Substrate` | 0.4 | A signaling molecule, ion, or hormone (calcium, serotonin) |
| `Receptor` | 0.7 | A molecular target (NMDA receptor, calcium channel) |

### Edge Types (14)

Organized into four complexity tiers:

**Foundational (0.0–0.1)** — What does it do?
| Edge | Min | Description |
|------|-----|-------------|
| `acts_on` | 0.0 | Ingredient influences a body system |
| `via_mechanism` | 0.1 | Relationship is mediated by a mechanism |
| `affords` | 0.0 | Enables a therapeutic property |
| `presents_in` | 0.1 | Symptom manifests in a system |
| `modulates` | 0.1 | Adjusts activity up or down (gain control) |

**Intermediate (0.3–0.5)** — How do things interact?
| Edge | Min | Description |
|------|-----|-------------|
| `contraindicated_with` | 0.3 | Safety conflict |
| `competes_with` | 0.4 | Competitive displacement at binding sites |
| `disinhibits` | 0.5 | Removes tonic inhibition |

**Advanced (0.6–0.8)** — Biochemical detail
| Edge | Min | Description |
|------|-----|-------------|
| `sequesters` | 0.6 | Binds and removes a substrate from availability |
| `releases` | 0.6 | Frees a sequestered substrate |
| `amplifies` | 0.7 | Cascade amplification |
| `desensitizes` | 0.8 | Reduces receptor sensitivity over time |

**Expert (0.85–1.0)** — Regulatory dynamics
| Edge | Min | Description |
|------|-----|-------------|
| `positively_reinforces` | 0.85 | Positive feedback loop |
| `gates` | 1.0 | Binary threshold gating |

### Edge Metadata

Every edge carries metadata beyond its type:

```rust
pub struct EdgeMetadata {
    pub confidence: f64,       // 0.0–1.0
    pub source: EdgeSource,    // Extracted, Confirmed, StructurallyEmergent
    pub iteration: u32,        // Which loop pass created this
    pub epoch: u32,            // Ontology version (for re-evaluation)
    pub llm_agreement: Option<f64>,
    pub extra: HashMap<String, String>, // Open for future dimensions
}
```

Confidence is assigned by curriculum stage: Foundational = 0.7, Relational = 0.85. The `epoch` field tracks when the complexity lens changes, so older edges can be re-evaluated with richer vocabulary.

---

## Complexity Lens

**Crate:** `graph-service` — `crates/graph-service/src/lens.rs`

The lens is a continuous `f64` from 0.0 to 1.0 that determines which node and edge types are visible. This is the key design decision that prevents advanced concepts from leaking into simple explanations.

### How It Works

Each type has a `min_complexity()` threshold. The lens compares:

```rust
pub fn can_see_edge(&self, edge_type: &EdgeType) -> bool {
    edge_type.min_complexity() <= self.level
}
```

### Named Presets

| Preset | Level | Visible Edges | Visible Nodes |
|--------|-------|---------------|---------------|
| `fifth_grade()` | 0.15 | 5 foundational | 5 basic |
| `tenth_grade()` | 0.50 | 8 (+ intermediate) | 6 (+ Substrate) |
| `college()` | 0.80 | 12 (+ advanced) | 7 (+ Receptor) |
| `graduate()` | 1.00 | 14 (all) | 7 (all) |

Custom values work: `ComplexityLens::new(0.35)` sees `contraindicated_with` (0.3) but not `competes_with` (0.4).

### Enforcement at Two Layers

1. **Prompt layer** — `extraction_system(&lens)` only teaches the LLM about visible types. A 5th-grade prompt never mentions "Substrate" or "competes_with."
2. **Parser layer** — `parse_triples(raw, Some(&lens))` rejects any triple using types above the lens, even if the LLM ignores the prompt constraints. This is the hard enforcement.

---

## Knowledge Graph

**Crate:** `graph-service` — `crates/graph-service/src/graph.rs`

A wrapper around `petgraph::Graph<NodeData, EdgeData, Directed>`.

### Key Operations

| Method | Description |
|--------|-------------|
| `add_node(NodeData)` | Adds or returns existing (deduplicates by name) |
| `find_node(&str)` | Case-insensitive lookup |
| `add_edge(src, tgt, EdgeData)` | Adds a directed edge |
| `outgoing_edges(idx)` | All `(target_idx, &EdgeData)` pairs |
| `all_nodes()` | All node indices for iteration |
| `node_count()` / `edge_count()` | Graph size |

Nodes are deduplicated by lowercase name. Edges are deduplicated by (source, target, edge_type) in the extraction parser, not in the graph itself.

---

## LLM Client

**Crate:** `llm-client` — `crates/llm-client/`

A provider-agnostic trait:

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
    fn provider_name(&self) -> &str;
    fn model_name(&self) -> &str;
}
```

### Providers

| Provider | Feature Flag | Env Var |
|----------|-------------|---------|
| Mock | (always available) | — |
| Anthropic Claude | `anthropic` | `ANTHROPIC_API_KEY` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` |

The mock provider uses substring matching for test determinism:

```rust
MockProvider::new("mock", "mock-v1")
    .on("5th grader", "Magnesium helps muscles relax...")
    .on("muscles relax", "magnesium|Ingredient|affords|muscle relaxation|Property")
    .with_default("magnesium|Ingredient|affords|general health|Property")
```

---

## Extraction Pipeline

**Crate:** `extraction` — `crates/extraction/src/`

Converts LLM prose into typed graph triples.

### Flow

```
LLM prose
  → extraction_prompt() builds the user message
  → extraction_system(&lens) builds the system message (lens-filtered)
  → LLM returns pipe-delimited triples
  → parse_triples() validates format, types, and lens compliance
  → ExtractionParser writes triples into the graph
```

### Triple Format

```
subject_name|SubjectType|edge_type|object_name|ObjectType
```

Example:
```
magnesium|Ingredient|affords|muscle relaxation|Property
magnesium|Ingredient|acts_on|muscular system|System
```

### Parser Rules

- Max 5 triples per extraction (prevents hallucination runaway)
- All names lowercased
- Deduplicates within each batch
- Lines without `|` are silently skipped (handles LLM preamble)
- Unknown types produce a warning, not a crash
- Lens violations produce a warning and skip the triple

---

## NSAI Loop

**Crate:** `nsai-loop` — `crates/nsai-loop/src/`

The core orchestrator. Runs the full seed → gap-fill → comprehension cycle.

### Architecture

```
NsaiLoop::run(nutraceutical, graph)
│
├── Step 1: Seed
│   Ask: "Explain to a 5th grader what {X} does as a supplement"
│   Extract response into graph
│
├── Step 2: Gap-filling (up to max_gap_iterations)
│   │
│   ├── analyzer::find_gaps(graph)
│   │   Identifies: LeafNode, NoMechanism, IndirectSystem
│   │
│   ├── For each gap (up to max_gaps_per_iteration):
│   │   Ask a targeted question about the gap
│   │   Extract response into graph
│   │
│   └── If graph unchanged → stop early
│
└── Step 3: Comprehension check
    Summarize graph as plain English
    Ask LLM to rephrase in different words
    Re-extract the rephrase
    Compare: edges_confirmed vs edges_new
```

### Gap Types

| Gap Kind | Detection | Question Generated |
|----------|-----------|-------------------|
| `LeafNode` | Node with no outgoing edges | "Tell me more about {node} in relation to {supplement}" |
| `NoMechanism` | Property node with no incoming `via_mechanism` edge | "Why is {supplement} connected to {property}?" |
| `IndirectSystem` | System node connected only through another system | "How does {supplement} directly affect {system}?" |

### Configuration

```rust
LoopConfig {
    max_gap_iterations: 3,      // default
    max_gaps_per_iteration: 5,  // default
}
```

### Comprehension Check

The comprehension check is a self-consistency test. It:

1. Converts the current graph into plain English sentences (e.g., "magnesium affords muscle relaxation")
2. Asks the LLM to explain the same information using completely different words
3. Re-extracts from the rephrase
4. Counts how many edges were confirmed (already in graph) vs. new (the LLM added something)

High confirmed-to-new ratio = stable understanding. Many new edges = the LLM has more to say (potential for another iteration).

---

## Event System

**Crate:** `event-log` — `crates/event-log/src/`

Every operation emits structured events for full observability.

### Event Types

| Event | When |
|-------|------|
| `LlmRequest` | Before each LLM call (includes prompt, provider, model, stage) |
| `LlmResponse` | After each LLM call (includes response, latency, token usage) |
| `LlmError` | LLM call failed |
| `ExtractionInput` | Raw text entering the extraction parser |
| `ExtractionOutput` | Parsed nodes, edges, and warnings |
| `GraphNodeMutation` | Node added to graph |
| `GraphEdgeMutation` | Edge added to graph (includes confidence) |
| `GapAnalysis` | Gaps detected (list of gap types + graph size) |
| `ComprehensionCheck` | Rephrase prompt/response + edge comparison stats |
| `LoopIteration` | End of each loop phase (seed, gap_fill, comprehension) |

### Sinks

| Sink | Use |
|------|-----|
| `JsonlSink` | Writes to `.jsonl` file with timestamps and correlation IDs |
| `MemorySink` | In-memory storage for tests (`events_for(correlation_id)`) |

Every event carries a `correlation_id` (UUID) that ties all events from a single pipeline run together.

---

## Curriculum Agent

**Crate:** `curriculum` — `crates/curriculum/src/`

Generates grade-appropriate questions for three levels:

| Level | Complexity | Example Question |
|-------|-----------|-----------------|
| 5th Grade | Foundational | "Explain to a 5th grader what magnesium does" |
| 10th Grade | Relational | "Explain to a 10th grader how magnesium works in the body" |
| College | Relational | "Explain to a college sophomore the biochemistry of magnesium" |

Currently the NSAI loop handles its own seed question at 5th grade level. The curriculum agent is available for future multi-level escalation once the single-level architecture is proven.

---

## Design Decisions

### Why a continuous complexity dial instead of discrete grade levels?

A discrete enum (`FifthGrade | TenthGrade | College`) would require special-case logic for each level and couldn't handle "between" levels. The continuous float lets us:
- Dial complexity precisely (e.g., 0.35 sees contraindications but not competition)
- Add new types without modifying existing enum variants
- Track when the dial changes (epoch system) for re-evaluation

### Why enforce the lens at both prompt AND parser layers?

The prompt layer is *guidance* — it tells the LLM what vocabulary to use. But LLMs don't always follow instructions. The parser layer is *enforcement* — it rejects any triple that uses types above the current complexity, regardless of what the LLM generated. Belt and suspenders.

### Why fill gaps at the same grade level before escalating?

Original design was to escalate immediately. But if a 5th-grade explanation has gaps, asking a 10th-grade question about those gaps produces 10th-grade answers that can't be extracted at the 5th-grade lens level. Better to exhaust the current vocabulary first, prove understanding with the comprehension check, then escalate.

### Why pipe-delimited triples instead of JSON?

LLMs are more reliable with simple, repetitive formats. Pipe-delimited lines have fewer failure modes than JSON (no bracket matching, no escaping, no nesting). The format is trivially parseable and the LLM can produce it with near-zero formatting errors.

### Why affordance-based reasoning?

"Magnesium cures insomnia" is a diagnosis. "Magnesium affords sleep quality" is an affordance — it describes what the supplement enables the body to do, without making medical claims. This satisfies the legal constraint while preserving the semantic richness needed for graph reasoning.
