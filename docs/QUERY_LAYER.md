# Query Layer — Final Design (Three-Model Consensus)

*Reviewed by Claude, Gemini, and Grok (2026-03-24). This is the synthesized design.*

## What Exists Today

### Graph Primitives (graph-service/src/graph.rs)
- **KnowledgeGraph** backed by SurrealDB embedded (RocksDB)
- Nodes deduplicated by slugified name, 14 node types, 14 edge types organized by complexity threshold
- Single-hop traversal: `outgoing_edges(node)`, `incoming_edges(node)`
- Lookup: `find_node(name)`, `find_node_or_alias(name, merge_store)`, `nodes_by_type(type)`

### Complexity Lens (graph-service/src/lens.rs)
- Continuous 0.0–1.0 dial gating which node/edge types are visible
- Presets: 5th_grade (0.15), 10th_grade (0.5), college (0.8), graduate (1.0)
- Currently enforced only at extraction time — **query layer extends enforcement to read path**

### Source Tracking & Quality (graph-service/src/source.rs)
- Quality tiers derived from observations: Deduced < Speculative < SingleProvider < MultiProvider < CitationBacked
- `edges_by_quality()` returns all edges with computed tiers in a single grouped query

### Merge Table (graph-service/src/merge.rs)
- `resolve(name)` → canonical name (single-hop alias resolution)
- CUI mappings for UMLS grounding

### Ontology Topology — Affordance-Based Reasoning
**Symptom → Supplement is always indirect (legal constraint):**
```
Symptom →[presents_in]→ System ←[acts_on]← Ingredient
Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient
```
No `relieves` edge type exists. This is structural, not a convention.

---

## Final Design

### Module: `graph-service/src/query.rs`

### Traversal Strategy: Pattern-Based (Not Generic BFS)

All three models agreed: recommendation queries use **structured pattern matching**, not generic BFS. Generic traversal is YAGNI until a real use-case demands it.

Patterns are encoded as a small enum, not hardcoded in method bodies:

```rust
/// A named traversal pattern through the ontology.
/// Each variant defines the edge types and directions for each hop.
enum RecommendationPattern {
    /// Symptom →[presents_in]→ System ←[acts_on]← Ingredient
    DirectSystem,
    /// Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient
    ViaMechanism,
}
```

`ingredients_for_symptom` runs all applicable patterns (filtered by lens visibility), merges results, and deduplicates by ingredient. Adding future patterns (e.g., via Property) means adding an enum variant — the public API doesn't change.

### Core Types

```rust
/// A single step in a traversal path
enum PathStep {
    Node { index: NodeIndex, data: NodeData },
    Edge { data: EdgeData, direction: EdgeDirection },
}

enum EdgeDirection { Forward, Reverse }

/// A discovered path through the graph with a composite score
struct TraversalPath {
    steps: Vec<PathStep>,
    score: f64,
    explanation: Vec<String>,  // human-readable per-hop fragments
}

/// Controls traversal visibility and filtering
struct QueryConfig {
    lens: ComplexityLens,
    min_quality: Option<EdgeQuality>,
    max_depth: usize,              // max edges in a path (default: 4)
    min_confidence: Option<f64>,   // skip edges below this threshold
    max_paths_per_result: usize,   // top-N paths per ingredient (default: 3)
}

/// A recommendation result grouped by ingredient
struct RecommendationResult {
    ingredient: NodeData,
    paths: Vec<TraversalPath>,
    best_score: f64,
    weakest_quality: EdgeQuality,
    contraindications: Vec<TraversalPath>,  // proactively included, empty if none
}
```

### QueryEngine

```rust
struct QueryEngine<'a> {
    graph: &'a KnowledgeGraph,
    source: &'a SourceStore,
    merge: &'a MergeStore,
    quality_map: HashMap<(String, String, String), EdgeQuality>,  // pre-loaded
}

impl QueryEngine {
    /// Build engine with eager quality map (one DB call).
    async fn new(graph, source, merge) -> Self;

    /// "What ingredients address this symptom?"
    /// Runs all RecommendationPatterns, filters by lens/quality,
    /// groups by ingredient, attaches contraindications.
    async fn ingredients_for_symptom(
        &self, symptom: &str, config: &QueryConfig,
    ) -> Vec<RecommendationResult>;

    /// "What ingredients act on this system?"
    async fn ingredients_for_system(
        &self, system: &str, config: &QueryConfig,
    ) -> Vec<RecommendationResult>;

    /// "What does this ingredient do?" (forward BFS)
    async fn effects_of_ingredient(
        &self, ingredient: &str, config: &QueryConfig,
    ) -> Vec<TraversalPath>;
}
```

No separate `contraindications_for` method. Contraindications are proactively checked inside every recommendation query and attached to results. Safety is a filter, not an afterthought.

### Path Scoring

**Three-model consensus:** raw confidence product punishes longer paths unfairly. Use geometric mean.

```
path_score = geometric_mean(confidences) × quality_bonus × length_bias
```

- **geometric_mean**: `(c1 × c2 × ... × cn) ^ (1/n)` — normalizes for path length
- **quality_bonus** (weakest-link multiplier):
  - Deduced: 0.5, Speculative: 0.7, SingleProvider: 1.0, MultiProvider: 1.2, CitationBacked: 1.5
- **length_bias**: `1.0 + (lens_level - 0.5) × 0.25 × (path_length - 2)`
  - At lens=0.15 (5th grade), len=4: `1.0 + (-0.35 × 0.25 × 2)` = 0.825 (penalizes length)
  - At lens=1.0 (graduate), len=4: `1.0 + (0.5 × 0.25 × 2)` = 1.25 (rewards detail)
  - Neutral at lens=0.5, length=2

Additionally, the lens controls **how many results** to return: lower complexity = fewer, more certain results. Higher complexity = more results including mechanistic alternatives.

### Quality Integration: Eager Map

Pre-load quality for all edges at `QueryEngine::new()` via a single `edges_by_quality()` call. Store as `HashMap<(source_node, target_node, edge_type), EdgeQuality>`.

- Graph is not huge — even 10k edges is trivial in RAM
- Quality rarely changes at query time (only on new observations)
- Avoids N per-edge DB hits during traversal

**v2 upgrade path:** materialize quality as a DB column on edge records, enabling SurrealDB-native quality filtering. Not needed for v1.

### Explanation Generation

Each `TraversalPath` carries `explanation: Vec<String>` — one human-readable fragment per hop, generated during traversal:

```
["Muscle cramps", "presents in Muscular System", "Magnesium acts on Muscular System"]
```

Cheap to produce (just node names + edge types), prevents every consumer from reimplementing path-to-text.

### Decisions on Open Questions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Crate placement | `query.rs` inside `graph-service` | All dependencies (Lens, SourceStore, MergeStore) are already there. Split only if >1k LOC. |
| DB-native vs app-level traversal | Application-level (Rust) | Lens + quality filtering is custom logic. DB is a fast adjacency list; thinking happens in Rust. |
| Path deduplication | Group by ingredient, top-3 paths per ingredient | Preserves path diversity ("recommended for 2 reasons") without overwhelming callers. |
| Contraindications | Proactive, inside `RecommendationResult` | One query = one safe answer. Caller never forgets to check. |
| Generic BFS | Not shipping in v1 | No real use-case yet. Add `custom_traversal(start, edge_pattern, config)` when needed. |

### Known Gaps (Not Blocking v1)

1. **Effect direction (upregulate/downregulate):** The `extra` metadata map on edges can carry this, but no edges have it yet. Ontology expansion — not a query layer concern. Tracked for roadmap.

2. **Lens ingestion mismatch:** Edges extracted at lens=0.8 are invisible at query lens=0.3. This is correct behavior (the lens gates what you see), but worth documenting for users who ask "why is this missing?"

3. **Inverse index performance:** `incoming_edges()` currently does a SurrealDB query filtered on `out = $node`. If this becomes slow at scale, add a SurrealDB index on the `out` field. Profile before optimizing.
