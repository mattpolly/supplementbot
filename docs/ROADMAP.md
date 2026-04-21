# Supplementbot Architecture Insights

## Biological Regulatory Forces → Edge Type Mapping

The eight fundamental regulatory forces in the human body map almost 1:1 to the ontology's edge types:

| Biological Force | Edge Type(s) |
|---|---|
| Modulation (gain control) | `modulates` |
| Competitive displacement | `competes_with` |
| Disinhibition (removing tonic brakes) | `disinhibits` |
| Sequestration / temporal gating | `sequesters`, `releases` |
| Cascade amplification | `amplifies` |
| Desensitization / adaptation | `desensitizes` |
| Positive feedback (runaway loops) | `positively_reinforces` |
| Threshold / all-or-nothing gating | `gates` |

These are genuinely distinct regulatory principles with different mathematical properties. Homeostasis uses all of them, not just excitatory/inhibitory signaling.

## Symptom → Supplement Traversal (Intake Direction)

When building the intake/chat component (Chief Complaint → HPI → ROS), do NOT add a direct `Ingredient → relieves → Symptom` edge. That's a medical claim and flattens the reasoning.

Instead, traverse the graph indirectly:
```
Symptom → presents_in → System ← acts_on ← Ingredient
```
With `Property` and `Mechanism` nodes filling in the *why*. This lets the system explain its reasoning ("cramps present in the muscular system, magnesium acts on the muscular system via NMDA receptor modulation, which affords muscle relaxation") rather than asserting "take magnesium for cramps."

This chain of reasoning is what distinguishes the system from a lookup table and keeps it legally safe under the affordance model.

## Symptom Node Gap

`Symptom` currently has only one edge type (`presents_in`). There's no typed path from a symptom back to an ingredient unless the LLM happens to generate an `affords` edge with a relief-oriented Property. This needs to be addressed when building the intake direction — but the indirect traversal above is the correct solution, not adding a direct `relieves` edge.

## Reverse Graph Traversal

`petgraph::Graph<_, _, Directed>` already supports `neighbors_directed(node, Direction::Incoming)`, so reverse traversal is free. When intake time comes:
- Add a symmetric `incoming_edges(idx)` method to the `KnowledgeGraph` wrapper (trivial, ~5 lines)
- The harder problem is multi-hop path-finding and **ranking competing paths** — if magnesium, calcium, and potassium all `acts_on` the muscular system, the graph gives three candidates
- Ranking likely comes from: edge confidence scores, mechanism specificity, and number of independent paths converging on the same ingredient
- Don't build this now; the traversal is free, the ranking is where the real complexity lives

## Traversal Path Preference Across Complexity Levels

When the graph contains both a simple 5th-grade edge and a detailed graduate-level decomposition of the same relationship, the traversal algorithm must choose which path to follow. Example: `magnesium → affords → sleep quality` (1-hop, 5th grade) vs. `magnesium → acts_on → NMDA receptor → via_mechanism → GABAergic inhibition → affords → sleep quality` (3-hop, graduate).

**Design:** The complexity lens already solves the easy case — at low complexity, graduate-level intermediate nodes are invisible, so only the simple path survives. At high complexity, both paths are visible and the algorithm should prefer the longer, more detailed path (specificity preference). The consumer wants the mechanistic decomposition, not the shortcut.

**Implementation strategy (when building the query/intake layer):**
1. Find all paths between endpoints (bounded by max hops)
2. Filter out paths where any intermediate node is below the current complexity threshold
3. Among surviving paths, prefer the one with the most intermediate nodes (richest explanation)
4. Use epoch as a secondary tiebreaker — higher-epoch edges are more refined

**UI opportunity:** When both a simple and detailed path exist, the interface can offer progressive disclosure: show the simple explanation first, with a "see mechanism" expand option that reveals the full chain.

See also: CONCERNS.md §6 for the full problem statement and escape hatches.

## Ingredient-to-Ingredient Edges (Synergy/Stacks)

Real-world formulations (e.g., Bluebonnet Super Quercetin) contain ingredients that interact with *each other*, not just with body systems:
- Bromelain increases quercetin bioavailability (potentiation)
- Vitamin C regenerates oxidized quercetin (reactivation)
- Multiple flavonoids (hesperidin, rutin, citrus bioflavonoids) cover adjacent inflammatory pathways

This requires:
- New edge types like `potentiates` or `enhances_bioavailability` (complexity ~0.6+)
- Allowing `Ingredient → edge → Ingredient` paths
- Updated extraction prompts that teach ingredient-to-ingredient relationships
- Updated gap analysis to detect multi-ingredient patterns (e.g., two ingredients share a system node but have no direct edge)

**Not needed now.** The current architecture does not block this — the triple format `subject|Type|edge|object|Type` already supports it, petgraph doesn't care, and the parser doesn't enforce type-pair restrictions. It's purely additive: new edge types, new prompt text, new gap detection rules.

## Gap Analysis Warning

Current gap types (`LeafNode`, `NoMechanism`, `IndirectSystem`) implicitly assume ingredient-outward topology. When ingredient-to-ingredient edges are added later, a node richly connected to other ingredients but with no system/mechanism edges would be incorrectly flagged as a leaf. Revisit gap detection at the same time as adding new edge types.

## Source Tracking Layer and Synonym Resolution

### Relational Source Tables (Built — 2026-03-22)

The graph owns topology; a parallel relational layer in SurrealDB (same embedded DB) owns the provenance audit trail. The `SourceStore` shares the same `Surreal<Db>` connection as the `KnowledgeGraph`.

**Tables:**
- **`node_source`** — one row per observation of a node: which provider, which model, when, correlation ID
- **`edge_source`** — one row per observation of an edge: provider, model, timestamp, confidence, source tag (Extracted/StructurallyEmergent/Deduced/Confirmed), observation type (created/confirmed)
- **`edge_citation`** (Built — 2026-04-21) — PubMed references (PMID, sentence, confidence) keyed by ingredient name, 22,952 citations across 19 ingredients

**Query capabilities (built):**
- `observations_for_edge(src, tgt, type)` — full history for any edge
- `provider_agreement(src, tgt, type)` — how many distinct providers observed an edge
- `multi_provider_edges()` — all edges confirmed by 2+ providers
- `total_node_observations()` / `total_edge_observations()` — aggregate counts

**Integration:** The `ExtractionParser` automatically records node and edge observations during extraction. Every node creation records a `NodeObservation`. Every edge creation records an `EdgeObservation` with type "created". Every duplicate edge detection records an `EdgeObservation` with type "confirmed". The `NsaiLoop` passes `SourceStore` through to both the main parser and the speculative parser.

**Portability:** The JSONL event log remains the portable source of truth. The SurrealDB source tables are materialized projections — disposable, rebuildable from the event log. If SurrealDB doesn't scale, we replay the event log into a new backend.

**Framing:** The graph started as a **speculative KG** — built from LLM extraction with no external validation. As of 2026-04-21, edges for all 19 ingredients are backed by PubMed citations from SuppKG (22,952 citations total). Edges with only LLM observations remain speculative; edges with literature citations in `edge_citation` are citation-backed. The source tables enable this distinction and the `CitationBacked` quality tier is now operational.

Graph confidence becomes a computed aggregate over source rows rather than a one-time assignment. Two providers independently extracting the same edge is stronger evidence than one — this is the path to real cross-provider validation.

### Edge Quality Tiers via Source Layer (Built — 2026-03-22)

**Decision:** Edge quality (speculative vs. confirmed vs. proven) is computed from source observations, not stored as a separate field on the edge. The graph topology stays complete — no "quarantine table" or separate speculative/proven graphs. Instead, consumers query the source layer with a minimum quality threshold.

**Quality tiers (weakest → strongest):**

| Tier | Criteria | Example |
|------|----------|---------|
| `Deduced` | Only `system:forward_chain` observations | Forward-chained `affords` shortcut |
| `Speculative` | Only `StructurallyEmergent` source tags from a single LLM | Topology-driven observation validated by one provider |
| `SingleProvider` | `Extracted` by exactly one LLM provider | Anthropic extracted `magnesium → acts_on → nervous system` |
| `MultiProvider` | `Extracted` by 2+ independent providers | Both Anthropic and Gemini extracted the same edge |
| `CitationBacked` | (Built — 2026-04-21) Confirmed by PubMed via SuppKG | Edge has a literature citation in the `edge_citation` table |

**Query API:**
- `edges_by_quality()` — classify all edges by tier
- `edges_at_quality(min)` — filter to edges at or above a threshold

**Why not a quarantine table?** The whole graph is speculative right now (all LLM-sourced). A separate quarantine table would split the topology, making traversal harder and requiring promotion logic. The source layer already tracks provenance — quality is a derived property, not stored state. When the intake/chat layer is built, it filters by `edges_at_quality(SingleProvider)` or higher. When PubMed extraction arrives, `CitationBacked` edges are simply those with rows in the `citations` table.

### Merge Table for Synonym Resolution

A non-destructive `node_alias` table records that two nodes are equivalent without modifying either node in the graph. This allows:
- Querying through aliases (treat "muscle relaxation" and "muscle rest" as one concept)
- Soft merge → hard merge promotion when confidence is high
- Undo capability (delete the alias row to un-merge)

**Detection** uses two tiers:
1. **Embedding similarity** — store embeddings on nodes, query for same-type pairs above 0.90 cosine similarity. Auto-alias above 0.95, flag for review between 0.80–0.95.
2. **LLM-as-judge** — three-way classification (same / related / independent) for the ambiguous middle zone. Result stored in merge table so the call only happens once per pair.

**Critical ordering:** Synonym resolution must run BEFORE inference (forward chaining, induction, abduction). Unresolved synonyms cause duplicate deductions and undercount inductive patterns.

SurrealDB stores embeddings natively alongside nodes — vector similarity queries require no external vector store.

## Clinical Intake Maps to Complexity Lens

The standard medical interview structure maps naturally to the lens:
- **Chief Complaint** = 5th-grade level ("I can't sleep")
- **HPI** = relational/intermediate ("started when I began working nights, caffeine makes it worse")
- **ROS** = system-by-system sweep filling in what the patient didn't volunteer

Could potentially run the lens in reverse — start at low complexity to match CC to broad system/property nodes, then escalate as the conversation gathers detail.

## Neurosymbolic Reasoning Roadmap

Supplementbot currently implements three of five classical reasoning modes. Here's the path to all five:

### Deduction: Forward Chaining (Built — Step 3 in NSAI Loop)

Pure symbolic reasoning — no LLM needed. If the graph contains `A → via_mechanism → M` and `M → affords → P`, the system deduces `A → affords → P` automatically. Runs after gap-fill but before comprehension check, so deduced edges are included in self-consistency validation.

- Walks all `via_mechanism → affords` two-hop paths
- Skips chains where the shortcut `affords` edge already exists
- Confidence = min(premise_a, premise_b) — weakest link principle
- Edges tagged `Source::Deduced`
- Each deduction emits a `ForwardChain` event with both premises and the conclusion

Future extensions: additional deduction rules beyond `via_mechanism + affords`, and optional LLM validation of deduced edges before insertion.

### Abduction: Speculative Inference (Built — Phase 4)

Already operational. The speculative engine observes graph topology (shared systems, shared properties) and proposes the best explanation — abductive reasoning. The LLM validates. Edges tagged `Source::StructurallyEmergent` at 0.5 confidence.

### Induction: Structural Observations (Built — Phase 3)

Already operational. `SharedSystem`, `SharedProperty`, `SharedMechanism`, `ConvergentPaths`, and `MechanismOverlap` observations generalize from specific instances to patterns. Pure inductive reasoning over graph topology.

### Bayesian Updating: Confidence Evolution (Not Yet Built)

Current confidence is assigned once and never updated. True Bayesian updating would:
- Start with a prior (0.5 for speculative, 0.7 for extracted)
- Increase when a second provider independently extracts the same edge
- Increase when the comprehension check confirms the edge
- Decrease if a provider contradicts it
- Use the `LlmAgreement` field already on `EdgeMetadata`

This turns confidence from a label into a signal that improves with evidence.

### Analogy: Structural Similarity (Not Yet Built)

Two ingredients with similar graph fingerprints (same systems, similar mechanism types, overlapping properties) are "analogous." If ingredient A has an edge that structurally-similar ingredient B lacks, the system proposes it by analogy. Example: if magnesium and zinc both act on the immune system via immune cell proliferation, and magnesium also acts on the nervous system, the system hypothesizes that zinc might also affect the nervous system — and asks the LLM.

This requires:
- A graph similarity metric (Jaccard similarity on neighbor sets, or graph embedding)
- Analogy-specific prompts ("B is similar to A in these ways. A also does X. Does B?")
- A new `Source::Analogical` tag

### The Five-Mode Pipeline

A single NSAI loop iteration could eventually run all five:
1. **Extract** (neural) — LLM teaches, graph learns
2. **Deduce** (symbolic) — forward chain guaranteed inferences
3. **Induce** (symbolic) — find structural patterns across ingredients
4. **Abduce** (neural + symbolic) — speculative inference validates topology-driven hypotheses
5. **Update** (symbolic) — Bayesian confidence adjustment based on all evidence this iteration

With analogy running periodically as ingredients accumulate.

## External Critique: Gemini's Feedback (2026-03-22)

Gemini reviewed the full architecture brief (CONFIRMATION.md). Here's what was actionable vs. what missed the mark.

### Accepted: Context/Condition Nodes

Gemini identified a genuine gap: supplement effects are often state-dependent ("magnesium absorption is better on an empty stomach," "vitamin D synthesis requires sun exposure," "MTHFR polymorphism affects folate metabolism"). The current ontology has no way to express conditions on edges.

**Plan:**
- Add a `Context` node type (min_complexity ~0.3–0.4, intermediate tier)
- New edge type `conditional_on` linking an existing edge to a Context node
- This is a form of **reification** — making a statement about a statement
- Not needed at 5th grade level, but required before college-level extraction where "it depends" is 80% of the answer

**Complexity:** This is the hardest ontology addition because it breaks the simple triple model. A conditional edge is really a hyperedge: `(A → acts_on → B) conditional_on C`. Options:
1. **Logic nodes** — a synthetic node representing the compound statement, with edges to its components
2. **Edge metadata** — store the condition in the `extra` HashMap (simple but untyped)
3. **N-ary relations** — SurrealDB can model this with nested RELATE statements

Option 1 is the most graph-native. Option 2 is the cheapest. Decide when we actually need it.

### Accepted: Triple-Level UUIDs for Source Tracking

Each extracted triple should get a UUID at parse time, before it enters the graph. This UUID becomes the foreign key linking graph edges to source table rows. Currently, source records reference edges by (source_node, target_node, edge_type) composite key plus correlation ID.

**Implementation:** Add a `triple_id: Uuid` field to `EdgeMetadata` (or to the `extra` map). Generate it in `ExtractionParser::extract_sentence()`. The source layer references this ID. *(Not yet implemented — the current composite key approach works but is fragile.)*

### Accepted: Supernode Awareness in Gap Analysis

As the graph grows, nodes like "immune system," "inflammation," and "nervous system" will accumulate hundreds of edges. The gap analyzer should not waste iterations asking about high-degree nodes — they're already well-connected.

**Plan:**
- Add a degree threshold to gap analysis: skip nodes with > N incoming edges (N ~ 20–50)
- Or: rank gaps by inverse degree, so low-connectivity gaps get priority
- This also helps speculative inference — don't speculate about supernodes, the combinatorial explosion is wasteful

### Accepted: Weight of Evidence over Pure Bayesian

Gemini suggested Weight of Evidence (WoE) over pure Bayesian updating, because true Bayesian requires well-grounded priors. A WoE framework assigns weights by source type:

| Source Type | Weight |
|---|---|
| Meta-analysis / systematic review | 1.0 |
| Randomized controlled trial | 0.8 |
| Observational study | 0.6 |
| LLM general knowledge (current) | 0.4 |
| Speculative inference | 0.2 |

This is more tractable than true Bayesian updating and maps naturally to the `citations` table in the source layer. When we shift from LLM-as-source to PubMed-as-source, each citation carries a study type that determines its weight.

**Refinement of the Bayesian Updating section above:** Keep the confidence-as-aggregate concept but ground weights in evidence type rather than computing true posteriors.

### Rejected/Deferred: Splitting Mechanism into Pathway + Action

Gemini suggested Mechanism is a "God Object." This misreads our implementation — our Mechanism nodes are already specific ("calcium channel blocking," "NMDA receptor modulation," "immune cell proliferation"). They're naturally scoped by the extraction prompts. The real problem isn't the type breadth; it's synonym proliferation within Mechanism, which the merge table addresses.

If Mechanism does become unwieldy at scale, splitting it is additive (new node types, update lens thresholds) and doesn't require rearchitecting.

### Rejected: Multi-Typed Nodes (for now)

Gemini suggested nodes should have roles rather than a single type. This adds significant complexity to the ontology, lens filtering, and type-pair validation — all for a problem ("energy production" typed as both Mechanism and Property) that we've seen exactly twice in 32 nodes. The merge table + source layer will surface type disagreements. Revisit if it becomes a real problem at scale.

### Noted: Affordance Model Legal Refinement

Gemini correctly noted that "affordance" is a linguistic shield, not a legal one — the FDA/FTC looks at intended use, not vocabulary. The indirect traversal path helps, but the UI layer must anchor on Structure/Function language ("supports healthy sleep cycles" not "affords sleep quality"). This is a UI/copy concern, not an architecture concern. The graph structure is fine; the presentation layer needs legal review when built.

---

## External Critique: Grok's Feedback (2026-03-22)

Grok's review was sharper and more domain-informed than Gemini's, citing production KGs (NP-KG, SuppKG, GENA) and biomedical standards (BEL, ChEBI, GO, BioPAX). Here's the triage.

### Accepted: Contradiction Detection as a Sixth Reasoning Mode

Grok identified a genuine gap: we have no way to handle two edges with opposite polarity on the same triple. When PubMed sources conflict ("Vitamin D increases calcium absorption" vs. a study showing decreased absorption under specific conditions), the graph currently just stores both with no reconciliation.

**Plan:**
- Add a `Source::Contradicted` tag or a contradiction flag on `EdgeMetadata`
- Contradiction detection runs as part of the source layer: when a new edge contradicts an existing one (same source/target, opposite effect), flag both for review
- This becomes critical when we shift to PubMed extraction — literature genuinely conflicts
- Could be the sixth mode in the pipeline, running after Bayesian/WoE updating

### Accepted: Canonical Grounding for Synonym Resolution

Grok suggested grounding high-confidence merges against external ontologies (ChEBI for chemicals, GO for biological processes, UMLS for medical terms). This is stronger than pure embedding similarity — if two node names both map to the same ChEBI ID, they're definitively the same concept.

**Plan:**
- Add optional canonical ID fields to node metadata (e.g., `chebi_id`, `go_id`, `umls_cui`)
- Use these as the first tier of synonym detection (exact match = guaranteed merge)
- Embedding similarity becomes the second tier for nodes without canonical IDs
- LLM-as-judge remains the third tier for ambiguous cases
- This also enables interoperability with other biomedical KGs

### Accepted: Type-Aware Similarity Thresholds

Grok noted that Symptom names are fuzzier than Receptor names. The 0.80/0.95 cosine thresholds should vary by node type:

| Node Type | Auto-merge | Review zone | Distinct |
|---|---|---|---|
| Receptor | > 0.98 | 0.90–0.98 | < 0.90 |
| Substrate | > 0.95 | 0.85–0.95 | < 0.85 |
| Mechanism | > 0.92 | 0.80–0.92 | < 0.80 |
| Property/Symptom | > 0.90 | 0.75–0.90 | < 0.75 |

Receptor and Substrate names are precise (NMDA receptor, serotonin); Property and Symptom names are fuzzy (muscle relaxation vs. muscle rest). Tighter thresholds for precise types, looser for fuzzy ones.

### Accepted: Leaf-Node Terminal Flag (Built — 2026-03-22)

System and Property nodes are valid leaf nodes — they're targets, not sources. Grok suggested a "terminal" flag to suppress gap-filling on these. Simpler than the inverse-degree approach from Gemini.

**Implementation (done):** The gap analyzer checks `is_terminal` based on node type. System nodes with incoming `acts_on` edges are terminal. Property nodes with incoming `affords` edges are terminal. Only flagged as gaps if they have zero incoming edges of the appropriate type. See `crates/nsai-loop/src/analyzer.rs`.

### Accepted: Periodic Re-teach at Higher Lens (Epoch Scheduling)

Low-grade edges (`modulates` instead of `competes_with`) become stale as the lens escalates. Grok recommended scheduling periodic "re-teach" epochs.

**Plan:** When the lens escalates (e.g., 5th → 10th grade), identify all edges created at the lower epoch. For each, re-ask the extraction question at the new lens level. If the re-extraction produces a more specific edge type, update the original. The epoch field on `EdgeMetadata` already tracks this — the infrastructure exists, we just need the trigger.

### Accepted: Output Filter Layer for User-Facing Safety

Both Gemini and Grok converged on this: the graph is safe internally, but any user-facing query interface must enforce Structure/Function language + FDA disclaimer. Grok went further: add a mandatory output filter that rewrites paths to pure S/F phrasing, plus a human review queue for paths touching Symptom nodes.

**Plan:** When building the intake/chat layer:
- All graph traversal results pass through a rewriting layer before display
- Symptom-touching paths get extra scrutiny (flag for review or require explicit disclaimer)
- Never surface raw edge types or node names to end users without S/F rewriting
- The `Symptom → presents_in → System ← acts_on ← Ingredient` traversal is the structural defense; the output filter is the presentation defense

### Noted but Deferred: Ontology Expansion (Pathway, Gene, Biological Process, Cell/Microbiota)

Grok recommended adding Pathway, Gene/Protein, Biological Process, Cell Type, and Microbiota node types, citing NP-KG and GENA as precedents. This is directionally correct for a production system but premature for our current scope.

**Our position:**
- We're operating at 5th grade level with 4 ingredients. Adding Gene/Protein and Biological Process requires college+ complexity lens
- The ontology is designed to be additive — new node types don't break existing ones
- When we escalate to college level and shift to PubMed extraction, these types become necessary
- Microbiota is a genuinely important gap (nutrient-microbiome interactions are bidirectional and underrepresented)
- **Sequence:** First prove the source layer and PubMed extraction work, then expand the ontology to accommodate what the literature actually contains

### Noted but Deferred: BEL-Style Predicates

Grok recommended switching from our custom edge types to BEL (Biological Expression Language) predicates (`increases`, `decreases`, `causes`, `prevents`, `hasAgent`, `hasProduct`) for interoperability. This is a valid point for production but:

- BEL predicates are flatter than our regulatory-force-based types (BEL's `increases` collapses modulation, amplification, and disinhibition)
- Our complexity-gated types are a deliberate design choice — they map to distinct mathematical properties of homeostatic regulation
- Interoperability can be achieved via a mapping layer (our types → BEL predicates) rather than replacing our ontology
- **When:** Build the mapping layer when we need to import/export to external KGs

### Noted but Deferred: Richer Extraction Format (JSON or BEL)

Grok suggested pipe-delimited triples limit expressiveness and recommended JSON objects or BEL statements with qualifier maps. The concern is real for qualifiers (dose, condition, directionality), but:

- Our pipe format has near-zero parse failures across two providers and hundreds of extractions
- JSON error rates with LLMs are meaningfully higher (bracket matching, escaping, nesting)
- Qualifiers can be handled via the `extra` HashMap on EdgeMetadata, populated by a second-pass extraction
- **When:** Consider upgrading when we need inline qualifiers that can't be post-processed. The Context node approach (from Gemini's feedback) may be a better graph-native solution than inline qualifiers.

### Rejected: Replace Continuous Complexity with 4 Discrete Levels

Grok recommended discrete tiers (Elementary / Intermediate / Advanced / Expert) over the continuous float, arguing it creates "false precision." Same argument as Gemini, same rebuttal:

- We already use discrete presets externally (`fifth_grade()`, `tenth_grade()`, etc.)
- The float is the internal representation; `min_complexity` thresholds on types are the enforcement mechanism
- No code path compares 0.35 vs. 0.36 — the float is compared against fixed thresholds
- A discrete enum would require modifying variants every time we add a type between existing tiers
- Both Grok and Gemini got confused by the representation and missed that the system already behaves like tiered buckets

### Rejected: PubMed Extraction as Immediate Priority

Grok's #1 recommendation was "immediately implement PubMed extraction + multi-source validation." This is the right *direction* but wrong *timing*:

- The current system is a learning project for neurosymbolic AI, not a production KG
- PubMed extraction requires: API integration, abstract parsing, study-type classification, citation management — each a significant feature
- The extraction pipeline is designed to support this shift (change knowledge source, keep extraction logic)
- **Sequence:** Source tracking layer (done) → Synonym resolution → PubMed extraction. The source infrastructure is now in place so PubMed data has somewhere to land with proper provenance.

### Meta-Observation: Grok vs. Gemini Critique Styles

Grok was domain-informed (cited NP-KG, SuppKG, GENA, BEL, ChEBI, FDA regulations) and focused on what production systems actually do. Gemini was architecture-focused and identified structural gaps (Context nodes, supernodes). Together they covered both the "does the design hold up theoretically" and "does this match what real systems look like" angles.

The most valuable feedback from both was convergent: contradiction detection, output safety filtering, canonical grounding for synonyms, and the eventual need for richer ontology types. The least valuable was convergent too: both suggested replacing the continuous complexity dial with discrete levels, both misunderstanding that the system already uses thresholds.

---

## General Architecture Validation

The following design decisions are already protecting future evolution:
- Continuous complexity dial (not discrete enum) allows precise tuning and new types without modifying variants
- Epoch system enables re-evaluation when the lens changes
- Open `extra: HashMap<String, String>` on edge metadata accommodates future dimensions
- Dual enforcement (prompt guidance + parser rejection) prevents advanced concepts from leaking into simple explanations
- Affordance-based reasoning ("affords sleep quality" not "cures insomnia") keeps semantics rich while avoiding medical claims

**Strategy: ship the single-ingredient pipeline, prove it works, then widen the lens.**

---

## Citation Strategy: Multi-Source Ingredient Knowledge Base (2026-04-20)

### The Problem

SuppKG was our sole source of PubMed-backed citations for the chat UI. It has 56,635 nodes, 595,222 edges, and 1.2M citations across 326k unique PMIDs — but its CUI namespace is frozen to UMLS 2006AA and iDISK v1's internal `DC*` identifiers. Modern UMLS CUIs don't match. Of our initial 19 test ingredients, only 6 could be resolved via hardcoded CUI overrides. The remaining ~13 had zero citations despite having abundant mentions in SuppKG's own sentence data.

We investigated CTD, iDISK 2.0, UMLS API, SemMedDB (deprecated), and DrugCentral. None bridged the gap. See `docs/PROBLEMS.md` for the full dead-end chronology.

### The Breakthrough: Sentence-Based SuppKG Mining

SuppKG's citations contain the ingredient name in the supporting sentence text, even when the CUI doesn't match. Searching sentences by ingredient name (with synonyms) instead of by CUI lookup recovers citations for all 19 ingredients:

| Ingredient | PMIDs via sentence search |
|---|---|
| probiotics | 8,519 |
| fish oil | 6,490 |
| calcium | 5,980 |
| vitamin d | 3,385 |
| iron | 3,226 |
| zinc | 2,770 |
| vitamin c | 1,517 |
| magnesium | 1,203 |
| gaba | 976 |
| quercetin | 823 |
| turmeric | 648 |
| vitamin b complex | 461 |
| coq10 | 380 |
| melatonin | 347 |
| nac | 300 |
| alpha-lipoic acid | 260 |
| ashwagandha | 151 |
| rhodiola | 76 |
| theanine | 68 |

This approach scales to hundreds of ingredients without any CUI resolution.

### Multi-Source Ingredient Knowledge Base

Rather than depending on any single external KG, we build our own ingredient knowledge base that aggregates across multiple sources. Each ingredient stores multiple external identifiers as cross-references:

**Per-ingredient record:**
- Ingredient name (primary key) + synonyms/common names
- SuppKG 2006 CUI (for sentence-mining SuppKG v1 citations)
- Modern UMLS CUI (from UMLS API, for PubMed searches and interop)
- iDISK ID + iDISK UMLS CUI (for metadata: background, mechanism of action, safety, source material)
- CTD MeSH ID (for curated chemical-disease associations and their PMIDs)

**Data sources and what each contributes:**

| Source | What it gives us | Identifier |
|---|---|---|
| SuppKG v1 | PMIDs + supporting sentences + confidence scores + predicates (mechanism-level edges) | 2006 UMLS CUI / DC* CUI (bypassed via sentence search) |
| iDISK 2.0 | Ingredient metadata (background, mechanism of action, safety text), synonyms, common names | iDISK_ID + UMLS CUI |
| CTD | Curated chemical-disease associations with PMIDs, monthly updates | MeSH ID |
| UMLS API | Modern CUIs, semantic types, cross-references | Current UMLS CUI |
| PubMed E-utilities | Fresh literature search by ingredient name or MeSH term | PMID |

### Implementation Plan

**Phase 1: Sentence-based SuppKG mining (COMPLETE — 2026-04-21)**
- Two-phase resolution: CUI-based for 8 ingredients, batch sentence search for remaining 11
- Single-pass scan of 1.2M citations with per-target cap (5 per ingredient×target_cui pair)
- Batch DB dedup via `record_citations_batch()` — 1 query per ingredient instead of per citation
- Fixed `load_with_edgelist` to preserve v1 JSON edges (with PMIDs) when merging v2 edgelist
- Result: 22,952 citations across all 19 ingredients

**Phase 2: Ingredient registry with multi-source IDs (COMPLETE — 2026-04-21)**
- `IngredientRegistry` in `crates/graph-service/src/registry.rs` with curated search terms
- 19 ingredients hydrated with synonyms from iDISK and CTD
- Provides `search_terms_for()` returning curated terms or fallback to name+synonyms
- Used by both CUI resolution and sentence search phases

**Phase 2.5: Runtime citation display (IN PROGRESS — 2026-04-21)**
- Backend `gather_citations_for_response()` in `handler.rs` looks up citations for ingredients mentioned in each response
- Frontend `showCitations()` in `chat.js` renders a side panel with grouped, collapsible citation cards
- CSS grid layout (`1fr | 900px | 1fr`) keeps chat centered when citations panel appears
- Citations accumulate across turns (append, don't replace) with auto-scroll
- Relevance ranking: keyword matching against session symptoms/systems and response text
- Negative-evidence filter: skip sentences expressing lack of data, negative results, etc.
- **Open problem:** SuppKG `target_node` values don't map to our ontology's body systems, so relevance depends on crude sentence keyword matching. Ideal solution: thread the graph traversal path into citation lookup so citations match the specific edge that justified the recommendation.

**Phase 3: CTD integration**
- Use CTD's curated chemical-disease file for additional PMIDs per ingredient
- CTD edges are disease-level (not mechanism-level), so they cannot be surfaced as supplement claims directly
- Use CTD PMIDs as a discovery layer: fetch the abstract from PubMed, extract mechanism-level sentences via LLM
- This reframes disease-associated papers into legally safe mechanism/body-system language

**Phase 4: PubMed refresh pipeline**
- Using modern UMLS CUIs and MeSH IDs, run annual PubMed E-utilities searches per ingredient
- Fetch new abstracts published since last training run
- Extract mechanism-level supporting sentences (LLM-based)
- Append to citation store with provenance tracking
- This keeps the citation base growing without rebuilding the entire pipeline

### Key Design Decisions

- **Ingredient name is the primary key**, not any external CUI. External IDs are cross-references, not identity.
- **SuppKG is mined by sentence text**, not by CUI resolution. This sidesteps the 2006 UMLS version lock entirely.
- **CTD disease edges are never surfaced directly** to users. They are a PMID discovery source only. Mechanism-level sentences are extracted from the underlying papers. This maintains legal safety under the affordance model.
- **iDISK provides the vocabulary layer** (names, synonyms, background text) but not citations (it has no PMIDs).
- **Dual CUI storage** (2006 + current) preserves backward compatibility with SuppKG while enabling modern PubMed search and interoperability with other biomedical systems.

### Coverage Notes

- **CTD coverage**: 18/19 test ingredients matched. Missing: probiotics (category, not a chemical). CTD has strong coverage for common supplements but uses pharmaceutical names (Acetylcysteine, Thioctic Acid, Ubiquinone). The ingredient registry maps consumer names to CTD names.
- **iDISK coverage**: 7,876 dietary supplement ingredients. All 19 test ingredients found. Rich synonym/common name data.
- **SuppKG sentence coverage**: All 19 test ingredients found via sentence search. Coverage varies (probiotics: 8,519 PMIDs; theanine: 68 PMIDs) but every ingredient has at least some citations.
- **Probiotics** remains the hardest ingredient across all sources because it's a category. Strategy: search for major strain families (Lactobacillus, Bifidobacterium, Saccharomyces) and aggregate under the "probiotics" umbrella.