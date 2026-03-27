# What We Learned

Everything discovered while building Supplementbot — the surprises, the mistakes, the corrections, and the insights that shaped the architecture.

---

## Table of Contents

- [LLM Extraction Behavior](#llm-extraction-behavior)
- [Graph Quality and Deduplication](#graph-quality-and-deduplication)
- [Type System and Validation](#type-system-and-validation)
- [Comprehension Check](#comprehension-check)
- [Complexity Lens Design](#complexity-lens-design)
- [Architecture and Extensibility](#architecture-and-extensibility)
- [Source Tracking and Provenance](#source-tracking-and-provenance)
- [Provider Differences](#provider-differences)
- [Testing with Mocks vs Real LLMs](#testing-with-mocks-vs-real-llms)
- [Prompt Engineering](#prompt-engineering)
- [Speculative Inference](#speculative-inference)
- [Gap Analysis](#gap-analysis)
- [Reasoning Modes and Neurosymbolic Theory](#reasoning-modes-and-neurosymbolic-theory)
- [Design Philosophy](#design-philosophy)

---

## LLM Extraction Behavior

### Pipe-delimited format works remarkably well

Both Anthropic and Gemini produce valid pipe-delimited triples with near-zero formatting errors. The format's simplicity is its strength — no bracket matching, no escaping, no nesting. Lines that aren't triples (preamble, headers) are trivially filtered by checking for `|`. JSON would have been a constant source of parse failures.

### LLMs are inconsistent about node typing

The same concept gets typed differently across extractions. "Energy production" appears as `Mechanism` in one extraction and `Property` in another, even from the same provider in the same run. This isn't a bug in the LLM — the concept genuinely straddles both types depending on context ("energy production" as a process vs. as an outcome).

**Implication:** Any type-pair validation must tolerate this ambiguity. An allowlist approach (`affords` requires target = `Property`) rejects valid triples where the LLM typed the target as `Mechanism` instead. A denylist (only reject the clearly nonsensical) is more robust.

### LLMs generate synonyms aggressively

Without guidance, an LLM will produce "muscle relaxation", "muscle rest", "relaxation", "cramp relief", "muscle cramp relief", "cramp prevention", and "muscle pain relief" — seven nodes for essentially one concept. This happens because each extraction call is independent; the LLM doesn't know what vocabulary already exists.

**Solution:** Feed existing graph node names (with types) into the extraction prompt. The LLM naturally reuses "muscle relaxation" when it sees it's already in the vocabulary. This collapsed Anthropic's graph from 20 nodes to 10 without losing any genuine concepts.

### LLMs occasionally misuse edge types

Claude used `Ingredient → presents_in → System` (should be `Symptom → System`). The prompt description says `presents_in: Symptom → System` but the LLM ignored it. Prompt-layer guidance is necessary but not sufficient — parser-layer enforcement is required.

### Extraction temperature should be 0.0

We use `temperature: 0.0` for extraction to maximize consistency and reproducibility. The comprehension check uses `0.3` to encourage slight rephrasing variation.

---

## Graph Quality and Deduplication

### Node deduplication by name is essential

The graph wrapper deduplicates nodes by lowercase name. Without this, every extraction call would create duplicate "magnesium" nodes. This was one of the earliest design decisions and it's been validated repeatedly.

### Edge deduplication happens in the parser, not the graph

The `KnowledgeGraph` itself doesn't prevent duplicate edges. The `ExtractionParser` checks `edge_exists(source, target, edge_type)` before adding. This was a deliberate choice — the graph is a dumb container, the parser is the smart gatekeeper.

### Name normalization matters

Gemini produced `energy_production` and `energy production` as separate nodes, and `muscular_system` and `muscular system`. Adding underscore-to-space normalization (plus whitespace collapsing) in the parser eliminated this class of duplicates entirely.

### The dedup progression

| Run | Anthropic Nodes | Gemini Nodes | Issue |
|-----|----------------|-------------|-------|
| Baseline (no dedup help) | 20 | 14 | Synonym explosion |
| + vocabulary injection | 8 | 6 | Over-suppressed, too sparse |
| + typed vocabulary | 8 | 12 | Anthropic lost mechanisms |
| + balanced prompt + denylist + normalization | 10 | 10 | Clean, balanced graphs |

The key insight: vocabulary injection needs to *encourage* reuse without *discouraging* new concepts. The prompt must explicitly say "create new nodes for genuinely new concepts."

### Synonym detection is a pre-inference concern, not a cleanup step

There are three distinct situations that look like duplicates but require different handling:

1. **Synonymous nodes** — "muscle relaxation" and "muscle rest" are the same concept in different words. These need to be merged or aliased.
2. **Related but distinct nodes** — "sleep quality" and "relaxation" are different properties that happen to be related. These should remain separate but could have an edge between them.
3. **Same effect via different channels** — `magnesium → via_mechanism → muscle contraction regulation → affords → muscle relaxation` AND `magnesium → via_mechanism → calcium channel blocking → affords → muscle relaxation`. Two paths converging on the same node. The graph handles this correctly already (`ConvergentPaths` observations detect it).

The critical insight: unresolved synonyms corrupt inference. Forward chaining over a graph with synonym nodes produces duplicate deductions. Induction undercounts patterns when "muscle relaxation" and "muscle rest" are separate nodes, weakening coverage scores. **The synonym resolution layer must run before inference, not after.**

### Non-destructive merge table is the only safe approach

Never delete a node to resolve a synonym — if the merge is wrong, the graph is corrupted with no undo. Instead, a relational "merge table" records that two nodes are equivalent without modifying either. Queries traverse through aliases. Soft merges can be promoted to hard merges later when confidence is high.

Detection uses two tiers:
- **Embedding similarity** (cheap, fast) — cosine similarity on node name embeddings. Above 0.95 = auto-alias. Between 0.80–0.95 = flagged for review. Below 0.80 = distinct.
- **LLM-as-judge** (expensive, accurate) — only for the ambiguous middle zone. Three-way classification: same concept, related concepts, or independent concepts. The answer goes into the merge table so the LLM call only happens once per pair.

SurrealDB can store embeddings alongside nodes and do vector similarity queries natively — no separate vector store needed.

---

## Type System and Validation

### Allowlists are too rigid, denylists are just right

We started with an allowlist defining valid (source_type, target_type) pairs for each edge type. Five foundational edges had explicit rules. This rejected too many valid triples because of the node typing inconsistency described above.

Switching to a denylist that only rejects two clearly wrong patterns (`presents_in` must be `Symptom → System`, `acts_on` must be `Ingredient → System`) produced richer graphs while still catching the nonsensical edges. Everything else is allowed through.

**The principle:** It's easier to say what not to do than to enumerate everything that's allowed. This applies broadly to LLM output validation.

### Type-pair validation must check stored types, not just parsed types

When the parser validates a triple, it checks the types as written in the LLM's output. But the node might already exist in the graph with a *different* type (first-writer-wins on `add_node`). So `Ingredient → affords → energy production (Property)` passes the initial check, but the stored node is `Mechanism`. We added a post-insert recheck against the actual stored node types to catch this.

### Node type conflicts are a real problem

The same concept gets different types from different extractions. "Energy production" is sometimes `Mechanism`, sometimes `Property`. Currently, first-writer-wins. Future options:
- Track type disagreements and warn
- Majority-vote across extractions
- Allow nodes to have multiple types (but this complicates the ontology significantly)

---

## Comprehension Check

### The original comprehension math was completely broken

The first implementation calculated confirmed edges as `edges_added.len() - edges_new`. But `edges_added` only contained edges that were *actually inserted* into the graph — duplicates were silently skipped by the dedup check. So if the rephrase produced the same triples (the ideal case), `edges_added` was empty and confirmed was `0 - 0 = 0`.

**Fix:** Added `edges_confirmed` to `ExtractionSummary`. When the parser finds a triple that already exists in the graph, it now records it as confirmed instead of silently dropping it. The comprehension check uses this directly.

### 5/0 is the target comprehension score

Both Anthropic and Gemini consistently produce 5 confirmed / 0 new edges on the comprehension check after the quality fixes. This means the rephrase re-extracts to the same graph structure — the understanding is stable and self-consistent.

### Comprehension is a genuine self-consistency signal

The comprehension check works by:
1. Summarizing the graph as plain English
2. Asking the LLM to rephrase in different words
3. Re-extracting from the rephrase
4. Counting confirmed vs. new edges

A high confirmed-to-new ratio means the LLM's understanding is stable across phrasings. This is a real signal, not just a test formality. Before the quality fixes, comprehension was 0/0 (blind) or 3/2 (unstable). After fixes, it's consistently 5/0.

---

## Complexity Lens Design

### Continuous dial, not discrete enum

The original proposal had discrete grade levels (5th, 10th, College, Graduate). During development, we realized that regulatory forces like cascade amplification and positive feedback need finer-grained gating. A continuous 0.0–1.0 float with named presets (`fifth_grade() = 0.15`) gives us:
- Precise tuning (0.35 sees contraindications but not competition)
- New types without modifying existing variants
- Named presets for convenience, custom values for precision

### The lens prevents conceptual leakage

A 5th-grader shouldn't encounter "NMDA receptor desensitization." Without the lens, adding advanced types to the ontology would cause the LLM to shoehorn simple answers into graduate-level categories. The lens filters at both the prompt level (only teaches visible types) and the parser level (rejects types above the lens). This is the key architectural decision that allows the full ontology to exist while keeping each grade level clean.

### Regulatory forces genuinely map to edge types

The eight fundamental regulatory forces in homeostasis (modulation, competitive displacement, disinhibition, sequestration/release, cascade amplification, desensitization, positive feedback, threshold gating) map almost 1:1 to the ontology's edge types. These aren't arbitrary categories — they have different mathematical properties and the body uses all of them.

### The epoch system enables re-evaluation

Every edge records which ontology epoch it was created under. When the complexity lens changes (e.g., escalating from 5th to 10th grade), older edges can be re-evaluated with the richer vocabulary now available. This hasn't been exercised yet but the infrastructure is in place.

---

## Architecture and Extensibility

### The triple format doesn't restrict type pairs — and shouldn't

`subject|Type|edge|object|Type` supports any node-type-to-node-type combination. This is important for future ingredient-to-ingredient edges (`potentiates`, `enhances_bioavailability`). The parser doesn't enforce type-pair restrictions beyond the denylist, and the graph doesn't care what types are on either end of an edge.

### Open metadata map is essential

`EdgeMetadata.extra: HashMap<String, MetadataValue>` has been unused so far, but it's the escape hatch for future dimensions (dosage-dependence, delivery method, bioavailability, etc.) without schema changes. The JSON roundtrip preserves it correctly.

### The event system is invaluable for debugging

Every LLM call, extraction, and graph mutation is logged with a correlation ID. The log viewer with filters (`--filter extraction`, `--filter gap`, `--filter comprehension`) made it possible to trace exactly which LLM response produced which bogus edge. Without this, debugging the type-pair and dedup issues would have been guesswork.

### Affordance-based reasoning is legally safe AND semantically rich

"Magnesium affords muscle relaxation" avoids medical claims while preserving the semantic structure needed for graph traversal. The indirect path `Symptom → presents_in → System ← acts_on ← Ingredient` with `Property` and `Mechanism` nodes filling in the "why" is both legally safe and more explanatory than a direct `relieves` edge.

### Never add a direct Ingredient → relieves → Symptom edge

This was identified in the roadmap review. A direct `relieves` edge is a medical claim and flattens the reasoning. The indirect traversal through System, Property, and Mechanism nodes preserves the chain of reasoning that distinguishes this from a lookup table.

---

## Provider Differences

### Anthropic produces tighter, more conservative graphs

Across all runs, Anthropic (Claude Sonnet 4.6) generated fewer nodes and edges than Gemini. Its graphs are precise but can miss concepts — it lost all Mechanisms in one run when type-pair rules were too strict.

### Gemini produces broader, more varied graphs

Gemini (gemini-3-flash-preview) generates more nodes, more symptoms, more systems. It found skeletal system and bone strength that Anthropic didn't mention. But it's more prone to formatting inconsistencies (underscores in names, occasional type confusion).

### Both providers respect the pipe-delimited format

Neither provider had significant format compliance issues. Both occasionally produce preamble text ("Here are the triples:") but the parser handles this by skipping lines without `|`.

### Both providers converge to 5/0 comprehension

After the quality fixes, both providers consistently reach 5 confirmed / 0 new on the comprehension check. The self-consistency signal is provider-independent.

---

## Testing with Mocks vs Real LLMs

### Mock providers can't simulate extraction nuance

The mock provider uses substring matching: if the prompt contains "muscles relax", return this canned response. This works for testing the pipeline structure but can't simulate the nuanced behavior of real LLMs — synonym generation, type inconsistency, format variation, novel concept introduction.

**Key gap discovered:** The mock's comprehension test always worked perfectly because the canned extraction output was designed to match the pre-populated graph. Real LLMs rephrase differently enough that the comprehension check initially scored 0/0. The bug was only visible with real providers.

### Test with real LLMs early and often

Several bugs were invisible in mock tests:
- Comprehension check math (0/0 vs. real confirmed counts)
- Synonym proliferation (mocks return exact strings)
- Type inconsistency (mocks always type things the same way)
- `presents_in` misuse (mocks don't make semantic errors)
- Underscore normalization (mocks don't use underscores)

Running against Anthropic and Gemini in quick succession revealed all of these.

---

## Prompt Engineering

### The extraction prompt evolved significantly

| Version | What It Included | Result |
|---------|-----------------|--------|
| V1 | Node types, edge types, format rules | Worked but noisy |
| V2 | + existing node names | Deduped but over-suppressed |
| V3 | + node types in vocabulary | Better typing but still over-suppressed |
| V4 | + "create new nodes for genuinely new concepts" | Balanced: clean + rich |

### Showing types alongside names prevents type confusion

When the vocabulary just listed names (`muscle contraction inhibition`), the LLM sometimes guessed the wrong type. When types were included (`muscle contraction inhibition (Mechanism)`), the LLM consistently used the correct type.

### "Reuse existing" must be balanced with "create new"

The prompt must explicitly encourage both behaviors. Without "reuse existing names," you get synonym explosion. Without "create new nodes for genuinely new concepts," the LLM collapses everything into existing vocabulary and stops generating novel nodes.

### The system prompt structure matters

The extraction system prompt has a deliberate structure:
1. Role ("knowledge-graph extraction assistant")
2. Node types (lens-filtered)
3. Edge types (lens-filtered)
4. Existing graph vocabulary (with types)
5. Output format specification
6. Rules (including reuse + create balance)
7. Example input/output

The example is critical — it shows the LLM exactly what "correct" output looks like.

---

## Speculative Inference

### The complexity lens constrains speculative vocabulary — and that's visible

When speculative inference validated that zinc and magnesium compete for absorption, the LLM used `modulates` because `competitive_displacement` isn't available at 5th grade level. The relationship is real, but the edge type is imprecise. This is the complexity lens working as designed — the system can only express what its current vocabulary allows. When we escalate to 10th grade, the epoch system should allow re-evaluation of these edges with richer vocabulary.

**Implication:** Speculative edges at low complexity levels should be treated as "directionally correct but potentially imprecise." The 0.5 confidence tag already signals this, but the edge *type* may also need re-evaluation at higher levels, not just the edge's existence.

### Speculative inference discovers real biochemical relationships

The structural observation that magnesium, zinc, and vitamin D all act on the skeletal system led the LLM to validate:
- `magnesium → via_mechanism → calcium absorption` (magnesium is required for calcium absorption — true)
- `magnesium → via_mechanism → vitamin d activation` (magnesium is needed to convert vitamin D to its active form — true)
- `zinc → affords → bone mineralization` (zinc is involved in bone matrix formation — true)

These are genuine biochemical facts that were never directly asked about. The graph's topology suggested the questions, and the LLM confirmed them. This is the core value proposition of speculative inference: the graph reasons about what *might* be true based on structure, then the LLM validates.

### Ingredient-to-ingredient edges emerge naturally

Anthropic produced `zinc → modulates → magnesium` during speculative inference — the first ingredient-to-ingredient edge in the graph. This wasn't prompted or designed for; the LLM chose to express the zinc-magnesium absorption competition as a direct relationship between ingredients. The ontology supports this (no type-pair restriction on `modulates`), but it wasn't anticipated at 5th grade level.

### Observation counts grow nonlinearly with ingredients

| Ingredients | Observations (Anthropic) | Observations (Gemini) |
|-------------|--------------------------|----------------------|
| 2 (Mag+Zinc) | 7 | 7 |
| 3 (+ Vitamin D) | 12→14 | 17→19 |

Adding one ingredient more than doubled the observation count because structural patterns are combinatorial — every new ingredient can share systems, properties, and mechanisms with every existing ingredient. The `max_speculative_observations` cap (default: 3) is essential to prevent runaway LLM calls as the graph grows.

### Gemini finds more speculative patterns than Anthropic

With 3 ingredients, Gemini found 17 observations vs. Anthropic's 12. This is consistent with Gemini's broader extraction behavior — it produces more edges per run, which creates more structural overlaps for the observation engine to find. Neither is "better"; Anthropic's graph is tighter and Gemini's is richer.

### Speculative edges enrich the graph without polluting it

After two speculative runs (Anthropic + Gemini), the graph went from 24 edges to 44, with 7 of those being `StructurallyEmergent` at 0.5 confidence. The speculative edges are clearly distinguishable in the graph dump and don't interfere with the core extracted knowledge. The `Source` tag and lower confidence make it easy to filter or weight them differently in downstream queries.

### The second provider run adds fewer speculative edges

Anthropic added 4 speculative edges; Gemini's subsequent run added only 3 more. This is because Anthropic's speculative edges already filled some of the structural gaps that Gemini would have also found. The dedup check in the parser prevents double-adding. This is healthy — it means multi-provider speculative runs converge rather than diverge.

---

## Gap Analysis

### Current gap types assume ingredient-outward topology

`LeafNode` (no outgoing edges), `NoMechanism` (Property with no incoming via_mechanism), and `IndirectSystem` (System connected only through another system) all assume the graph grows outward from ingredients. This will need revisiting when ingredient-to-ingredient edges arrive — a node richly connected to other ingredients but with no system/mechanism edges would be incorrectly flagged.

### System and Property nodes are valid terminal nodes (fixed)

"Nervous system" and "muscular system" are targets, not sources — they receive `acts_on` edges but don't need outgoing edges to be useful. The gap analyzer originally flagged them as leaf nodes every iteration, creating unnecessary gap-filling questions that produced low-quality edges. **Fixed (2026-03-22):** System nodes with incoming `acts_on` edges and Property nodes with incoming `affords` edges are now recognized as valid terminals and skipped by the gap analyzer. Only System/Property nodes with zero incoming edges of the appropriate type are flagged as gaps.

### Gap-filling can be repetitive

When the LLM answers a gap-filling question, the extracted triples sometimes just restate existing edges in different words. The comprehension check catches this (high confirmed count), but the gap-filling iterations could be smarter about recognizing when an answer didn't actually add new structure.

---

## Reasoning Modes and Neurosymbolic Theory

### Supplementbot implements multiple classical reasoning modes

What started as "extract triples from an LLM" turns out to map precisely onto distinct reasoning modes from symbolic AI and philosophy of science:

| Reasoning Mode | What It Does | Where It Lives in Supplementbot |
|----------------|-------------|--------------------------------|
| **Deduction** | Given premises, derive guaranteed conclusions | Forward chaining: if `A → via_mechanism → M` and `M → affords → P`, then `A → affords → P` (built) |
| **Induction** | Observe patterns, generalize rules | `SharedSystem`/`SharedProperty` observations: "magnesium, zinc, and vitamin D all act on the immune system" |
| **Abduction** | Propose the best explanation for observations | Speculative inference: graph topology suggests a relationship, LLM validates whether it's real |
| **Bayesian updating** | Revise confidence as evidence accumulates | Confidence scoring (aspirational — currently assigned, not truly Bayesian) |
| **Analogy** | Transfer knowledge between structurally similar entities | Not yet built — discovering relationships between ingredients with similar graph topology |

### Speculative inference is abduction, not Bayesian inference

The speculative engine looks at graph structure and proposes the *best explanation* for observed patterns. When it sees that magnesium, zinc, and vitamin D all act on the skeletal system, it hypothesizes that they might interact — and asks the LLM to validate. This is textbook abductive reasoning: inference to the best explanation.

This is distinct from Bayesian inference, which would update a prior probability as new evidence arrives. Our confidence scores *look* Bayesian (numbers between 0 and 1) but aren't — they're assigned once and never updated based on accumulating evidence.

### The structural observations are induction

`SharedSystem`, `SharedProperty`, and `SharedMechanism` observations are inductive generalizations. The system observes specific instances (magnesium acts on immune system, zinc acts on immune system, vitamin D acts on immune system) and induces the general pattern ("these three ingredients share immune system involvement"). This is classic inductive reasoning — particular to general.

### Forward chaining is deduction

The next feature to build — symbolic constraint propagation — is pure deduction. If the graph contains `A → via_mechanism → M` and `M → affords → P`, the system can *deduce* `A → affords → P` without asking the LLM. This is guaranteed to be valid given the premises (though the premises themselves may be wrong, which is why LLM validation still matters).

### A mature system would use all five modes

A complete neurosymbolic reasoning system could use all five modes together:
- **Deduction** for guaranteed inferences from existing edges (forward chaining)
- **Abduction** for speculative edges proposed by graph topology
- **Induction** for cross-ingredient generalizations from structural observations
- **Bayesian updating** for confidence that improves as multiple sources confirm the same edge
- **Analogy** for discovering new relationships between ingredients with similar graph fingerprints

This is the real value of the neurosymbolic approach — no single reasoning mode is sufficient, but the combination of symbolic graph structure with neural LLM validation enables all of them.

### The LLM-as-source problem

The current system learns from LLMs, which means the provenance chain is circular: "we know this because an LLM said so, and we validated it by asking the LLM again." The comprehension check is self-consistency, not ground truth. This is fine for learning neurosymbolic AI, but a production system would need citable sources (PubMed, NIH fact sheets) as the knowledge source, with the LLM's role shifting from *source of knowledge* to *reader and structurer of knowledge*. The extraction pipeline would barely change — instead of "tell me about magnesium" you'd send "extract triples from this abstract."

---

## Source Tracking and Provenance

### The JSONL event log is the portable source of truth

When we built the source tracking layer (relational tables in SurrealDB for node/edge provenance), the key design question was: what happens if SurrealDB doesn't hold up? The answer: the JSONL event log already captures every mutation with full context (provider, model, source tag, correlation ID, timestamp). The SurrealDB source tables are materialized projections — disposable views that can be rebuilt from the event log. If we ever need to swap to Neo4j, Postgres, or anything else, we replay the log.

This is the architectural escape hatch that makes it safe to commit to SurrealDB now without backing into a corner.

### "Evidence" is a loaded word in supplement contexts

The source tracking layer was originally called "EvidenceStore" with `node_evidence` / `edge_evidence` tables. We renamed everything to "source" (`SourceStore`, `node_source`, `edge_source`) because "evidence" in the supplement space can be misconstrued as clinical evidence (meta-analyses, RCTs). Our source tables track *who said what and when*, not *what is clinically proven*. The naming matters for communicating intent — both to ourselves and to anyone reviewing the system.

### Speculative KG vs. Proven KG is the right framing

The current graph is a **speculative KG** — built from LLM extraction with no external validation. When edges are confirmed via PubMed, external KGs (NP-KG, SuppKG), or clinical data, they should graduate to a **proven KG**. The source tables enable this distinction: edges with only LLM-provider observations remain speculative; edges with literature citations become proven.

This framing was prompted by external critique (Gemini, Grok, Chat Claude all pushed toward grounding) and crystallized the relationship between our current system and the future evidence-based version. We're not building a half-finished proven KG — we're building a complete speculative KG with infrastructure to evolve.

### Cross-provider observation is cheap validation

Running extraction through both Anthropic and Gemini produces two independent observations of the same edge. The `SourceStore.multi_provider_edges()` query identifies edges observed by 2+ providers — these are meaningfully stronger than single-provider edges. This is the cheapest form of validation available and requires zero new infrastructure beyond what we already built. The source tracking layer turns provider diversity from an incidental feature into an exploitable asset.

### Source store shares the DB connection — no new infrastructure

The `SourceStore` takes a reference to the same `Surreal<Db>` handle used by `KnowledgeGraph`. Both live in the same embedded SurrealDB instance. No additional database, no separate connection, no configuration. The source tables are just additional tables in the same DB namespace. This means provenance tracking adds zero operational complexity.

### Edge confirmation is a distinct event from edge creation

When the parser encounters a triple that already exists in the graph, it now emits an `EdgeConfirmed` pipeline event (distinct from `GraphEdgeMutation`) and records the observation in the source table with type "confirmed". This distinction matters: creation is new knowledge, confirmation is reinforcement. The source table captures both, and the `provider_agreement` query counts distinct providers across both creation and confirmation observations.

---

## External Critique Learnings

### Gemini review (2026-03-22) — what landed and what didn't

We submitted the full architecture to Gemini for critique. Key takeaways:

**Context-dependent relationships are a real gap.** "Magnesium absorption is better fasted," "Vitamin D needs sun exposure," "MTHFR polymorphism affects folate metabolism" — these are conditional statements the graph can't express. This is a form of reification (making statements about statements) and the hardest ontology problem ahead. Not needed at 5th grade, critical at college level.

**Supernodes will break gap analysis.** Nodes like "immune system" and "inflammation" will accumulate hundreds of edges at scale. Gap-filling a high-degree node is wasteful. The fix is simple: rank gaps by inverse degree, or skip nodes above a threshold. Same applies to speculative inference — don't speculate about supernodes.

**Weight of Evidence is more tractable than pure Bayesian.** Gemini correctly pointed out that true Bayesian updating requires well-grounded priors we don't have. A Weight of Evidence framework that assigns weights by source type (meta-analysis > RCT > observational > LLM general knowledge > speculative) is simpler and maps naturally to the citations table. Keep confidence-as-aggregate but ground it in evidence type, not posterior computation.

**Triple-level UUIDs would improve the source foreign key problem.** Assigning a UUID to each triple at parse time would give the source layer a clean foreign key without depending on composite (source_node, target_node, edge_type) lookups. Currently the source tables use composite keys plus correlation IDs, which works but is fragile.

**Gemini overshot on several points:**
- Called Mechanism a "God Object" — but our Mechanism nodes are already specific ("NMDA receptor modulation," not "mechanism"). The real problem is synonym proliferation, which the merge table solves.
- Suggested multi-typed nodes — adds complexity for a problem we've seen twice in 32 nodes. The merge table + source layer will surface type disagreements if they become material.
- Called the continuous complexity dial "false precision" — but we already use tiered buckets (named presets) for logic. The float is internal representation; the thresholds are what matter.
- Called "affordance" merely a linguistic shield — partially fair, but missed that the indirect traversal path (`Symptom → System ← Ingredient`) is the real structural defense. The valid point is that the UI layer must use FDA Structure/Function language ("supports healthy sleep cycles") when presenting results.

**The meta-lesson:** External critique is most valuable when it identifies gaps you *can't see from inside the architecture* (context nodes, supernodes) and least valuable when it suggests redesigning things that work (Mechanism typing, the complexity dial). The "poor man's MoE" approach works best when you know which feedback to absorb and which to file.

### Grok review (2026-03-22) — sharper, more domain-informed

Grok cited production-grade supplement KGs (NP-KG, SuppKG, GENA) and biomedical standards (BEL, ChEBI, GO, BioPAX), grounding its critique in what real systems actually do. Much sharper than Gemini.

**Contradiction detection is a missing reasoning mode.** When PubMed sources conflict (and they will — nutrition literature is full of contradictory findings), we have no way to represent or resolve the conflict. Two edges with opposite polarity on the same triple just coexist silently. This becomes critical the moment we shift to literature-grounded extraction. It's arguably a sixth reasoning mode alongside the existing five.

**Canonical grounding beats embedding similarity for synonym resolution.** If two node names both map to the same ChEBI ID or GO term, they're definitively the same concept — no embedding comparison needed, no LLM-as-judge needed. External ontology IDs should be the first tier of synonym detection, with embeddings and LLM as fallback tiers. This also opens the door to interoperability with other biomedical KGs.

**Similarity thresholds should be type-aware.** "NMDA receptor" and "NMDA receptor complex" need a tight threshold (0.98) because receptor names are precise. "Muscle relaxation" and "muscle rest" need a looser threshold (0.90) because property names are fuzzy. One-size-fits-all thresholds will either over-merge precise types or under-merge fuzzy ones.

**System/Property leaf nodes need a terminal flag, not just degree-based skipping.** Rather than the inverse-degree approach (Gemini), Grok suggested marking certain node types as valid terminals. A System node with incoming `acts_on` edges is doing its job — it's a target, not a source. Simpler and more semantically correct than degree thresholds.

**The epoch re-teach mechanism needs an explicit trigger.** We built the epoch field on EdgeMetadata but never defined when re-evaluation happens. Grok pushed for scheduled re-teach: when the lens escalates, identify all lower-epoch edges and re-extract at the new complexity level. The `modulates` → `competes_with` upgrade for zinc-magnesium competition is the exact use case.

**Where Grok pushed too hard:**
- Recommended "immediately implement PubMed extraction" — right direction, wrong timing. Evidence layer and synonym resolution need to exist first so PubMed data has proper infrastructure to land in.
- Recommended switching to BEL-style predicates — BEL's `increases`/`decreases` are flatter than our regulatory-force-based types. A mapping layer for interoperability is better than replacing the ontology.
- Recommended richer extraction format (JSON/BEL) — our pipe format has near-zero parse failures. Qualifiers are better handled via Context nodes or the `extra` HashMap than inline format complexity.
- Same as Gemini: recommended discrete complexity levels over continuous float. Same rebuttal — the system already uses thresholds, the float is just internal representation.

**Grok's most valuable contribution:** Connecting our architecture to the existing landscape of biomedical KGs. Knowing that NP-KG and SuppKG already do literature-grounded extraction for natural products validates our planned direction and gives us concrete systems to study. The ChEBI/GO/UMLS grounding suggestion is the kind of domain knowledge you can only get from someone who knows the field.

### Poor Man's MoE: Meta-Learnings from External Critique

Running the same architecture brief through two different LLMs (Gemini and Grok) produced complementary feedback:

| Dimension | Gemini | Grok |
|---|---|---|
| **Focus** | Architecture / structure | Domain / production systems |
| **Best insight** | Context nodes, supernodes | Contradiction detection, canonical grounding |
| **Weakest point** | "Mechanism is a God Object" | "Immediately implement PubMed" |
| **Convergent (both said it)** | Replace continuous complexity with enum (wrong), output safety filter (right) |
| **Style** | Suggested redesigns | Cited precedents |

The process works. Key lessons:
1. **Ask for critique, not validation.** "Please critique this" got better results than "does this look good?"
2. **Triage ruthlessly.** About 40% of suggestions were actionable, 30% were deferred, 30% were rejected. Accepting everything would have been worse than accepting nothing.
3. **Convergent feedback is signal.** Both independently identified the output safety filter need and the complexity dial non-issue. When two models agree, pay attention — whether they're right or wrong.
4. **Domain-specific models add the most value.** Grok's citations of NP-KG and BEL standards were more useful than any architectural suggestion. The best external critique tells you what already exists in your problem space.

---

## Design Philosophy

### Start simple, prove it works, then widen

The system is scoped to 5th grade only. The lens, curriculum agent, and epoch system are all designed for multi-level operation, but we proved the single-level pipeline works before adding complexity. This principle — ship the narrowest useful thing, then expand — has been validated repeatedly.

### Don't back yourself into corners

From the earliest conversations, the design prioritized abstractions that could evolve:
- Continuous complexity dial instead of discrete enum
- Open metadata map instead of fixed fields
- Provider-agnostic LLM trait instead of hardcoded API calls
- Epoch-based versioning for re-evaluation
- No type-pair restrictions on the graph itself (only in the parser)

Each of these was a deliberate choice to avoid premature commitment.

### The LLM is the teacher, the graph is the student

The NSAI loop's structure reflects this: the LLM generates knowledge, the graph stores it, the gap analyzer generates questions, and the comprehension check tests understanding. All three levels of the reasoning hierarchy are now operational:

- **Level 1:** Direct extraction — LLM teaches, graph learns (seed + gap-fill) — *neural*
- **Level 2:** Structural inference — graph reasons about its own topology (find_observations) — *symbolic induction*
- **Level 3:** Speculative inference — graph proposes hypotheses, LLM validates them (run_speculative_inference) — *abduction (neural + symbolic)*
- **Level 4:** Forward chaining — graph deduces guaranteed inferences from existing edges — *symbolic deduction* (built)
- **Level 5:** Source tracking — provenance records which providers observed each node/edge, enabling cross-provider validation — *multi-source evidence* (built)
- **Level 6 (planned):** Bayesian/WoE updating — confidence evolves as source observations accumulate — *probabilistic reasoning*

Level 3 is where it gets interesting: the graph discovers that magnesium, zinc, and vitamin D all act on the skeletal system, then *asks the LLM* whether that shared connection means anything. The LLM confirms real biochemical relationships (calcium absorption, vitamin D activation) that were never directly asked about in the curriculum. See the [Reasoning Modes](#reasoning-modes-and-neurosymbolic-theory) section for the full theoretical framework.

### Observability is not optional

The event system was built early (Phase 2) and has been essential throughout. Every bug discovered during real-LLM testing was traced through the event log. The correlation ID pattern (one UUID per pipeline run) makes it possible to follow a single nutraceutical's journey from seed question to final graph.

### Legal constraints shape the architecture

"Never diagnose, never say cure" isn't just a disclaimer — it's an architectural principle. The affordance model (`affords`, `acts_on`, `presents_in`) was designed from the start to describe what supplements *enable the body to do* rather than what they *treat*. The indirect traversal path (`Symptom → System ← Ingredient`) was identified in the roadmap as the correct intake direction, specifically to avoid a direct `relieves` edge that would constitute a medical claim.

### Refactor early, not later

The SurrealDB migration replaced the entire storage layer (petgraph → SurrealDB embedded) while the codebase was still ~2000 lines across 8 crates. Every file that touched `KnowledgeGraph` needed changes: all graph operations became async, `&mut KnowledgeGraph` became `&KnowledgeGraph` (SurrealDB handles interior mutability), and `NodeIndex` changed from petgraph's type to a wrapper around `RecordId`. Despite touching every crate, the refactor was straightforward because:
- The `KnowledgeGraph` API surface was clean (10 methods)
- All callers were already async (LLM calls)
- Tests used the same API, just needed `KnowledgeGraph::in_memory()` instead of `KnowledgeGraph::new()`

Had we waited until 10 crates and 5000 lines, this would have been painful. The lesson: when you know a fundamental capability is needed (persistence), do it now while it's cheap.

### SurrealDB embedded is the right choice for this project

SurrealDB's embedded mode (RocksDB backend) gives us:
- **No infrastructure** — single directory on disk, no server to run
- **Native graph relations** — `RELATE` statement maps perfectly to our edge model
- **Full SurrealQL** — the same query language works whether embedded or server-deployed
- **In-memory mode for tests** — `Mem` backend keeps tests fast and isolated

The `RELATE` model is a natural fit because our edges already have typed metadata. A SurrealDB relation is `node:src->edge->node:tgt` with arbitrary fields — exactly our `EdgeData` with `edge_type` and `metadata`.

### First SurrealDB compile is brutal, incrementals are fine

Adding SurrealDB pulls in ~500 crates including the full database engine. First `cargo build` takes 6-10 minutes. After that, incremental builds only recompile your code (~2 seconds). This is a one-time cost worth paying for what you get.

### `SurrealValue` derive replaces serde for DB types

SurrealDB v3 uses its own `SurrealValue` derive macro instead of relying on serde's `Serialize`/`Deserialize` for records stored in the database. Types need both: serde for JSON serialization (event logs, Display) and `SurrealValue` for database storage. The `#[serde(...)]` field attributes don't work with `SurrealValue` — we worked around this for edge records by aliasing `in`/`out` fields in SurrealQL queries (`SELECT *, in AS source, out AS target FROM edge`).
