# Concerns

The hardest open problems in supplementbot, synthesized from external critique (Gemini, Grok, Chat Claude — 2026-03-22) and our own observations. These are not feature requests. These are the things that could silently degrade the system or block scaling.

Each concern includes escape hatches — approaches we believe can address it — roughly ordered from cheapest to most ambitious.

---

## 1. Complexity Explosion

**The problem:** Structural observations grow combinatorially with ingredients. 2 ingredients produce 7 observations. 3 produce 17. At 20 ingredients the observation space will be in the thousands, and each observation can trigger an LLM call for speculative validation. The `max_speculative_observations` cap doesn't prioritize — it just cuts off randomly from an exponentially growing pool.

This isn't just a cost problem. Random sampling from a huge observation space means the system *misses important observations by chance*. The cap that prevents runaway LLM calls also prevents discovery.

**Who flagged it:** Us (primary), Gemini (supernode bottleneck), Grok (scaling to 1000 ingredients).

**Escape hatches:**

1. **Observation ranking** (**done** — 2026-03-22) — Observations are now scored by: ingredient count, observation type weight (ConvergentPaths > SharedMechanism > SharedProperty > SharedSystem), and supernode dampening. The cap cuts the least interesting observations. See `crates/nsai-loop/src/structural.rs`.

2. **Tiered observation generation** (medium effort) — Don't run full induction on every loop iteration. Run lightweight observations (SharedSystem, SharedProperty) every time, but expensive ones (ConvergentPaths, MechanismOverlap) only when the graph has meaningfully changed since the last full pass. Track a "last observation epoch" and skip recomputation if the subgraph is stable.

3. **Supernode dampening** (**done** — 2026-03-22) — Nodes above degree 15 have their observation scores dampened by 0.7 (multiplied by 0.3). Integrated into the observation scoring function. See `crates/nsai-loop/src/structural.rs`.

4. **Batch speculative queries** (medium effort) — Instead of one LLM call per observation, batch related observations into a single prompt: "Given that magnesium, zinc, vitamin D, and omega-3 all act on the immune system, what are the most important interactions?" This reduces LLM calls from O(observations) to O(observation_clusters).

5. **Incremental observation** (harder) — Only compute observations involving *newly added* nodes/edges, not the entire graph. Maintain an observation cache and invalidate entries when their contributing nodes change.

---

## 2. Noise vs. Weak Signals

**The problem:** A speculative edge at 0.5 confidence could be a genuine discovery, an LLM hallucination, or a real relationship expressed with the wrong edge type. All three look identical in the graph. The system has no mechanism to distinguish signal from noise in speculative edges, and every speculative edge participates in forward chaining and further speculation — meaning noise propagates through every reasoning mode.

The gap analyzer compounds this: it flags System/Property nodes as leaf nodes, asks low-quality gap-fill questions, gets back edges that restate existing knowledge with different words, which create new nodes, which become new leaf nodes. A mild positive feedback loop that adds noise each iteration.

**Who flagged it:** Us (primary), Chat Claude (gap analyzer false positives are worse than framed), Grok (edge pruning).

**Escape hatches:**

1. **Terminal node flag** (**done** — 2026-03-22) — System nodes with incoming `acts_on` edges and Property nodes with incoming `affords` edges are doing their job. The gap analyzer now skips them. See `crates/nsai-loop/src/analyzer.rs`.

2. **Gap-fill staleness tracking** (**done** — 2026-03-22) — Nodes that have been asked about N times (default 2) without producing new edges are skipped. Counter resets when a gap produces results. See `crates/nsai-loop/src/loop_runner.rs`.

3. **Speculative edge quarantine** (**addressed** — 2026-03-22) — Rather than a separate candidate table, speculative edges are gated by two mechanisms: (a) **confidence decay** removes 0.05 per pass from unconfirmed speculative/deduced edges (floor 0.1), so unpromoted speculation fades; (b) **quality tiers** (`EdgeQuality::Speculative` < `SingleProvider` < `MultiProvider` < `CitationBacked`) let consumers filter speculative edges out of queries via `edges_at_quality()`. Citation backing via SuppKG can promote speculative edges to `CitationBacked` if PubMed evidence supports them.

4. **Cross-provider validation** (**infrastructure built** — 2026-03-22, Chat Claude's insight) — The `SourceStore` now records which provider observed every node and edge. `multi_provider_edges()` identifies edges confirmed by 2+ providers. `provider_agreement()` gives full details for any specific edge. The infrastructure is in place; the next step is using it to boost/gate confidence. See `crates/graph-service/src/source.rs`.

5. **Confidence decay** (**done** — 2026-03-22) — Speculative and deduced edges that are never confirmed by multiple providers lose 0.05 confidence per decay pass (floor at 0.1). Extracted edges are exempt. Multi-provider edges are exempt. Runs after cross-provider boosting in the CLI. See `crates/nsai-loop/src/confidence.rs`.

6. **Low-confidence edge pruning** (medium effort) — Periodically remove or archive edges below a confidence threshold. This keeps the active graph clean and prevents noise from compounding through reasoning modes. Pruned edges move to an archive table, not deleted — they can be restored if new evidence emerges.

---

## 3. Critical but Missed Edges (Ignorance)

**The problem:** The gap analyzer finds structural gaps (leaf nodes, missing mechanisms) but not conceptual gaps. It doesn't know that magnesium's role in DNA repair is important if nothing in the graph hints at DNA repair. The system can only discover relationships that are adjacent to what it already knows — it has no way to detect that entire domains are missing.

This is the hardest problem because you can't observe what isn't there. The graph looks complete by its own topology even when it's missing 90% of the relevant biology.

**Who flagged it:** Us (primary), Grok (ontology expansion, literature bias toward popular supplements).

**Escape hatches:**

1. **Coverage metrics** (**done** — 2026-03-22) — `CoverageReport` checks each ingredient for at least one `acts_on`, `via_mechanism`, and `affords` edge. CLI prints missing structural requirements. See `crates/nsai-loop/src/analyzer.rs`.

2. **External curriculum cross-reference** (medium effort) — Compare the graph against known supplement-system associations from reference sources (NIH Office of Dietary Supplements fact sheets, examine.com's structured data). If the reference says "magnesium is involved in 300+ enzymatic reactions" and our graph has 5 mechanisms, we know we're missing things. This is lighter-weight than full PubMed extraction.

3. **Negative-space analysis via analogy** (the planned reasoning mode) — Compare graph fingerprints of well-studied vs. less-studied ingredients. If magnesium has edges to 6 systems and zinc has edges to 3, the absence of zinc edges to those other systems is informative. Either zinc doesn't interact, or we haven't asked. The analogy reasoning mode is designed for exactly this — it identifies what *should* exist based on structural similarity.

4. **Targeted probing questions** (medium effort) — Instead of only asking the LLM open-ended questions derived from gaps, periodically ask "what important aspects of {ingredient} have we not discussed?" or "what body systems does {ingredient} affect that we haven't covered?" These meta-questions break out of the graph's current topology and let the LLM volunteer information the gap analyzer can't find.

5. **Literature coverage scoring** (requires PubMed) — When we shift to PubMed extraction, count abstracts per ingredient. If there are 5,000 papers on magnesium and we've processed 20, we have 0.4% coverage. This gives a concrete incompleteness metric rather than just structural completeness.

---

## 4. Pipeline Stage Ordering and Cascade Effects

**The problem:** The NSAI loop runs stages in sequence, but the dependency graph between stages is more complex than the linear order suggests. Chat Claude identified the critical question: what happens when a gap-fill response introduces a synonym node, which triggers forward chaining that deduces an edge, which speculative inference then builds on? Each stage can introduce artifacts that cascade through subsequent stages.

Current known ordering constraints:
- Synonym resolution must run before inference (documented)
- Forward chaining must run before comprehension (built)
- Speculative inference runs after comprehension (built)

Unknown/untested interactions:
- Gap-fill can introduce synonyms mid-pipeline (no synonym check between gap-fill iterations)
- Forward chaining on speculative edges can produce deduced edges with compounded uncertainty
- Speculative inference on deduced edges can produce speculation-of-deduction chains

**Who flagged it:** Chat Claude (primary — neither Gemini nor Grok pressed on this).

**Escape hatches:**

1. **Intra-iteration synonym check** (medium effort) — Run a lightweight synonym detection pass after each gap-fill extraction, not just at the start of the pipeline. This prevents synonym accumulation within a single iteration. Expensive if done with embeddings on every extraction; cheap if done as name-similarity heuristic (edit distance, shared tokens).

2. **Source-chain tracking** (medium effort) — When forward chaining deduces an edge, record *which* premises it used, including their Source tags. A deduced edge whose premises are both Extracted (0.7+) is solid. A deduced edge with a StructurallyEmergent premise is speculation-of-speculation and should inherit the lowest confidence in the chain (which it does via weakest-link, but the Source tag should also reflect this).

3. **Reasoning depth limits** (**done** — 2026-03-22) — `EdgeMetadata.reasoning_depth` tracks how many layers of inference produced an edge (0 = extracted, 1 = deduced/speculated from extracted, etc.). Forward chaining respects `MAX_PREMISE_DEPTH = 1` to break speculation → deduction → speculation cascades. See `crates/nsai-loop/src/forward_chain.rs` and `crates/graph-service/src/types.rs`.

4. **Pipeline contract tests** (**done** — 2026-03-22) — 6 integration tests in `crates/nsai-loop/tests/pipeline_contract.rs` validate: required event types are emitted, event ordering (seed → extraction → gap → comprehension), graph structure (expected nodes/edges), synonym CUI assignment, citation backing, and source tracking coverage. These make pipeline regressions observable.

---

## 5. Cross-Provider Validation (Infrastructure Built — 2026-03-22)

**The problem:** We have two independent LLM providers extracting knowledge about the same supplements, but we treat each run independently. An edge extracted by both Anthropic and Gemini is stronger evidence than one confirmed by comprehension check (which is just self-consistency within a single provider). We're not fully exploiting this yet.

**Who flagged it:** Chat Claude (primary — neither Gemini nor Grok mentioned it).

**What's built:** The `SourceStore` now records every node and edge observation with provider identity. The `ExtractionParser` automatically records provenance during all extraction (seed, gap-fill, comprehension, speculative). Key queries:
- `multi_provider_edges()` — all edges observed by 2+ distinct providers
- `provider_agreement(src, tgt, type)` — full observation details for any edge
- `observations_for_edge(src, tgt, type)` — complete history of who observed what, when

**What's still needed:**

1. **Provider intersection scoring** (**done** — 2026-03-22) — `boost_multi_provider_confidence()` applies +0.15 to edges confirmed by 2+ providers, capped at 1.0. See `crates/nsai-loop/src/confidence.rs`.

2. **Provider disagreement flagging** (small effort) — When one provider extracts `A → acts_on → B` and the other extracts `A → contraindicated_with → B`, that's a contradiction worth flagging. Use `observations_for_edge()` to detect conflicting edge types between the same node pair. This is the seed of the contradiction detection reasoning mode.

3. **Consensus extraction mode** (medium effort) — Run both providers on the same question, extract from both, and only insert edges that appear in both results. Edges unique to one provider go into a candidate/quarantine table. Configurable: `--consensus` flag for strict mode, default for union mode.

---

## 6. Traversal Path Preference Across Complexity Levels

**The problem:** When the graph contains both a simple 5th-grade edge (`magnesium → affords → sleep quality`) and a detailed graduate-level decomposition of the same relationship (`magnesium → acts_on → NMDA receptor → via_mechanism → GABAergic inhibition → affords → sleep quality`), which path does a traversal follow? A naive shortest-path algorithm prefers the 5th-grade edge every time, discarding the richer mechanistic explanation that a higher-complexity consumer actually wants.

This isn't a problem *yet* because we only have 5th-grade data. But as soon as we escalate the lens, both paths will coexist in the same graph, and every query that traverses from ingredient to property will face this choice.

**Status update (2026-03-25):** We now have 10th-grade data and the query layer implements lens-filtered traversal + `length_bias` scoring that prefers longer paths at higher lens levels. Escape hatches #1 and #2 are partially addressed. No longer blocked on data — remaining work is specificity preference and path deduplication.

**Who flagged it:** Us (primary — observed while discussing lens coexistence).

**Escape hatches:**

1. **Lens-filtered traversal** (**done** — 2026-03-24) — The query engine's pattern-based traversal only follows edges visible at the current lens level. At low complexity, graduate-level intermediates are invisible. At high complexity, all nodes are visible. See `crates/graph-service/src/query.rs`.

2. **Length bias scoring** (**done** — 2026-03-24) — `length_bias(lens_level, path_edge_count)` adjusts path scores based on complexity: at high lens levels, longer paths (richer explanations) get a bonus. At low lens levels, shorter paths are preferred. See `score_path()` in `query.rs`.

3. **Path deduplication** (medium effort) — Detect when a long path is a decomposition of a short path (same endpoints, intermediate nodes are subtypes or mechanisms of the short path's relationship). Tag the short path as "summarizes" the long path. This lets the UI offer both: "magnesium supports sleep quality (simple) — click to see mechanism."

4. **Epoch-aware ranking** (medium effort) — Paths composed of higher-epoch edges (created at higher grade levels) are more refined. Use epoch as a tiebreaker when multiple paths survive lens filtering. This naturally prefers graduate-level decompositions over 5th-grade shortcuts when both are visible.

---

## 7. Supernode "body" Pollutes Query Results

**The problem:** LLMs frequently emit "body" as a generic System node. This node accumulates edges from many ingredients and systems, creating a supernode that appears in nearly every traversal path. Unlike the observation-level supernode dampening (Concern #1, done), this directly degrades query results — paths through "body" are uninformative but score well because the node has high connectivity.

**Who flagged it:** Us (observed during query layer testing, 2026-03-24). Grok confirmed.

**Escape hatches:**

1. **Fix at extraction time** (**ready for next ingest**) — Add a post-extraction filter: if `node.name == "body" && node.type == System`, discard or remap. "Body" is not a real system — it's LLM vagueness. This is the cleanest fix: prevent pollution upstream.

2. **Degree-based specificity penalty in path scoring** (**ready for next ingest**) — High-degree intermediate nodes get a multiplier penalty. Defense-in-depth against future supernodes that sneak through extraction. Formula: `specificity = 1.0 - (0.05 × log(degree + 1))` for intermediate nodes, or a hard cap (degree > 20 → 0.6× multiplier).

---

## 8. ViaMechanism Traversal Pattern Never Fires

**The problem:** The query engine's `ViaMechanism` pattern (`Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient`) requires `Mechanism →[modulates]→ System` edges. The current extraction prompt does not elicit these edges. The graph has Mechanisms connected to Ingredients via `via_mechanism`, but no `modulates` edges connecting Mechanisms to Systems. The second traversal pattern is dead code against real data.

**Who flagged it:** Us (observed during query layer testing, 2026-03-24). Grok confirmed.

**Escape hatches:**

1. **Update extraction prompt** (**ready for next ingest**) — Explicitly teach `Mechanism →[modulates]→ System` relationships. Example: "NMDA receptor modulation modulates nervous system." This produces high-confidence, explicitly-stated edges.

2. **Forward-chaining inference rule** (**ready for next ingest**) — If `Ingredient →[acts_on]→ System` AND `Ingredient →[via_mechanism]→ Mechanism`, infer `Mechanism →[modulates]→ System`. This backfills missing links automatically from existing topology. Confidence = min(premise_a, premise_b), tagged `Source::Deduced`.

3. **Both** (recommended) — Extract where possible for higher quality, forward-chain the rest for coverage.

---

## 9. Quality Map Direction Bug (Fixed)

**The problem:** `weakest_quality_in_path()` and `quality_from_steps()` used the `EdgeDirection` flag to determine canonical source/target order for quality map lookups. But path node order varies by query type — `ingredients_for_system` builds paths starting from the ingredient, while `pattern_direct_system` builds paths starting from the symptom. The direction flag alone couldn't determine canonical order, causing reverse-traversed edges to look up the wrong key and default to `Deduced` quality.

**Who flagged it:** Us (observed during query layer testing, 2026-03-24).

**Resolution (2026-03-25):** `edge_quality()` now tries both `(a, b, edge_type)` and `(b, a, edge_type)` orderings against the quality map. Since each (source, target, edge_type) triple is unique, at most one ordering matches. This makes quality lookups direction-agnostic. Scores were already correct (pattern methods pass canonical order directly); only the displayed `weakest_quality` field was affected.

---

## Priority Assessment

| Concern | Severity | Effort | When |
|---|---|---|---|
| ~~Terminal node flag~~ | ~~Medium~~ | ~~Trivial~~ | **Done** (2026-03-22) |
| ~~Source tracking layer~~ | ~~High~~ | ~~Medium~~ | **Done** (2026-03-22) |
| ~~Gap-fill staleness~~ | ~~Medium~~ | ~~Small~~ | **Done** (2026-03-22) |
| ~~Coverage metrics~~ | ~~Medium~~ | ~~Small~~ | **Done** (2026-03-22) |
| ~~Observation ranking~~ | ~~High~~ | ~~Medium~~ | **Done** (2026-03-22) |
| ~~Cross-provider intersection scoring~~ | ~~High~~ | ~~Small~~ | **Done** (2026-03-22) |
| ~~Reasoning depth limits~~ | ~~Medium~~ | ~~Small~~ | **Done** (2026-03-22) |
| ~~Speculative edge quarantine~~ | ~~High~~ | ~~Medium~~ | **Addressed** (2026-03-22) — confidence decay + quality tiers gate speculative edges |
| ~~Supernode dampening~~ | ~~High~~ | ~~Medium~~ | **Done** (2026-03-22) |
| ~~Pipeline contract tests~~ | ~~Medium~~ | ~~Medium~~ | **Done** (2026-03-22) — 6 e2e tests in `crates/nsai-loop/tests/pipeline_contract.rs` |
| ~~Confidence decay~~ | ~~Medium~~ | ~~Medium~~ | **Done** (2026-03-22) |
| Traversal path preference | Medium (correctness) | Small–Medium | **Partially addressed** (2026-03-24) — lens filtering + length bias done; path dedup + epoch ranking remain |
| ~~Supernode "body" in queries~~ | ~~High (query quality)~~ | ~~Small~~ | **Fixed** (2026-03-25) — extraction filter rejects "body"/"human body" System nodes + degree-based specificity penalty in path scoring (>15 edges → 0.6× multiplier) |
| ~~ViaMechanism pattern dead~~ | ~~Medium (feature gap)~~ | ~~Small~~ | **Fixed** (2026-03-25) — `modulates` prompt description now teaches `Mechanism → System`; forward-chain rule deduces `M →[modulates]→ S` from `A →[acts_on]→ S` + `A →[via_mechanism]→ M` |
| ~~Quality map direction bug~~ | ~~Medium~~ | ~~Trivial~~ | **Fixed** (2026-03-25) |
