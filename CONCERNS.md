# Concerns

The hardest open problems in supplementbot, synthesized from external critique (Gemini, Grok, Chat Claude — 2026-03-22) and our own observations. These are not feature requests. These are the things that could silently degrade the system or block scaling.

Each concern includes escape hatches — approaches we believe can address it — roughly ordered from cheapest to most ambitious.

---

## 1. Complexity Explosion

**The problem:** Structural observations grow combinatorially with ingredients. 2 ingredients produce 7 observations. 3 produce 17. At 20 ingredients the observation space will be in the thousands, and each observation can trigger an LLM call for speculative validation. The `max_speculative_observations` cap doesn't prioritize — it just cuts off randomly from an exponentially growing pool.

This isn't just a cost problem. Random sampling from a huge observation space means the system *misses important observations by chance*. The cap that prevents runaway LLM calls also prevents discovery.

**Who flagged it:** Us (primary), Gemini (supernode bottleneck), Grok (scaling to 1000 ingredients).

**Escape hatches:**

1. **Observation ranking** (buildable now) — Not all observations are equally interesting. Score them by: number of ingredients involved (4-way SharedSystem > 2-way), confidence of contributing edges, novelty (does this observation involve a node we haven't speculated about yet?). The cap then cuts the least interesting observations, not random ones. The structural analyzer already sorts by significance — make that sorting smarter.

2. **Tiered observation generation** (medium effort) — Don't run full induction on every loop iteration. Run lightweight observations (SharedSystem, SharedProperty) every time, but expensive ones (ConvergentPaths, MechanismOverlap) only when the graph has meaningfully changed since the last full pass. Track a "last observation epoch" and skip recomputation if the subgraph is stable.

3. **Supernode dampening** (buildable now) — Nodes above a degree threshold (immune system, inflammation) contribute disproportionately to observation counts. Either exclude them from combinatorial observation generation, or weight their observations lower. A node connected to everything is informative about nothing.

4. **Batch speculative queries** (medium effort) — Instead of one LLM call per observation, batch related observations into a single prompt: "Given that magnesium, zinc, vitamin D, and omega-3 all act on the immune system, what are the most important interactions?" This reduces LLM calls from O(observations) to O(observation_clusters).

5. **Incremental observation** (harder) — Only compute observations involving *newly added* nodes/edges, not the entire graph. Maintain an observation cache and invalidate entries when their contributing nodes change.

---

## 2. Noise vs. Weak Signals

**The problem:** A speculative edge at 0.5 confidence could be a genuine discovery, an LLM hallucination, or a real relationship expressed with the wrong edge type. All three look identical in the graph. The system has no mechanism to distinguish signal from noise in speculative edges, and every speculative edge participates in forward chaining and further speculation — meaning noise propagates through every reasoning mode.

The gap analyzer compounds this: it flags System/Property nodes as leaf nodes, asks low-quality gap-fill questions, gets back edges that restate existing knowledge with different words, which create new nodes, which become new leaf nodes. A mild positive feedback loop that adds noise each iteration.

**Who flagged it:** Us (primary), Chat Claude (gap analyzer false positives are worse than framed), Grok (edge pruning).

**Escape hatches:**

1. **Terminal node flag** (**done** — 2026-03-22) — System nodes with incoming `acts_on` edges and Property nodes with incoming `affords` edges are doing their job. The gap analyzer now skips them. See `crates/nsai-loop/src/analyzer.rs`.

2. **Gap-fill staleness tracking** (buildable now) — If a gap-fill question for a node has been asked N times across iterations and never produced new edges, stop asking. Track `gap_fill_attempts` and `last_new_edge_from_gap_fill` per node. Diminishing returns = stop.

3. **Speculative edge quarantine** (medium effort) — Don't insert speculative edges directly into the graph at 0.5 confidence. Park them in a candidate table. They promote to real edges only when: (a) a second provider independently produces the same edge, (b) a subsequent extraction independently confirms it, (c) forward chaining from confirmed edges arrives at the same conclusion, or (d) a PubMed citation supports it. The `SourceStore` now provides the infrastructure: `multi_provider_edges()` can identify which speculative edges have been independently confirmed. This turns the source layer from an audit trail into a gating mechanism.

4. **Cross-provider validation** (**infrastructure built** — 2026-03-22, Chat Claude's insight) — The `SourceStore` now records which provider observed every node and edge. `multi_provider_edges()` identifies edges confirmed by 2+ providers. `provider_agreement()` gives full details for any specific edge. The infrastructure is in place; the next step is using it to boost/gate confidence. See `crates/graph-service/src/source.rs`.

5. **Confidence decay** (medium effort) — Speculative edges that are never independently confirmed should lose confidence over time (or over iterations). An edge at 0.5 that's never confirmed after 10 more ingredients are added is probably noise. An edge that gets confirmed twice is probably signal. Time/iteration-based decay naturally separates them.

6. **Low-confidence edge pruning** (medium effort) — Periodically remove or archive edges below a confidence threshold. This keeps the active graph clean and prevents noise from compounding through reasoning modes. Pruned edges move to an archive table, not deleted — they can be restored if new evidence emerges.

---

## 3. Critical but Missed Edges (Ignorance)

**The problem:** The gap analyzer finds structural gaps (leaf nodes, missing mechanisms) but not conceptual gaps. It doesn't know that magnesium's role in DNA repair is important if nothing in the graph hints at DNA repair. The system can only discover relationships that are adjacent to what it already knows — it has no way to detect that entire domains are missing.

This is the hardest problem because you can't observe what isn't there. The graph looks complete by its own topology even when it's missing 90% of the relevant biology.

**Who flagged it:** Us (primary), Grok (ontology expansion, literature bias toward popular supplements).

**Escape hatches:**

1. **Coverage metrics** (buildable now) — Define minimum structural completeness per ingredient. At minimum: at least one `acts_on`, at least one `via_mechanism`, at least one `affords`. If an ingredient is missing any, it's structurally incomplete regardless of what the gap analyzer says. This is a schema-level check, not a topology-level one.

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

3. **Reasoning depth limits** (buildable now) — Don't allow speculative inference to operate on Deduced edges, or at least don't allow it to speculate on edges that were themselves deduced from speculative premises. This breaks the speculation → deduction → speculation cascade. A simple depth counter on edge metadata (`reasoning_depth: 0` for extracted, `1` for deduced-from-extracted, `2` for speculated-from-deduced) would let any stage filter by depth.

4. **Pipeline contract tests** (buildable now) — Write integration tests that simulate the problematic cascades: inject a synonym during gap-fill, verify forward chaining doesn't produce duplicate deductions; inject a low-confidence speculative edge, verify it doesn't cascade into high-confidence deductions. These don't fix the problem but make it observable.

---

## 5. Cross-Provider Validation (Infrastructure Built — 2026-03-22)

**The problem:** We have two independent LLM providers extracting knowledge about the same supplements, but we treat each run independently. An edge extracted by both Anthropic and Gemini is stronger evidence than one confirmed by comprehension check (which is just self-consistency within a single provider). We're not fully exploiting this yet.

**Who flagged it:** Chat Claude (primary — neither Gemini nor Grok mentioned it).

**What's built:** The `SourceStore` now records every node and edge observation with provider identity. The `ExtractionParser` automatically records provenance during all extraction (seed, gap-fill, comprehension, speculative). Key queries:
- `multi_provider_edges()` — all edges observed by 2+ distinct providers
- `provider_agreement(src, tgt, type)` — full observation details for any edge
- `observations_for_edge(src, tgt, type)` — complete history of who observed what, when

**What's still needed:**

1. **Provider intersection scoring** (small effort) — Use `multi_provider_edges()` to boost confidence on edges confirmed by both providers (+0.15 or similar). Edges in only one provider's observation set get no boost. The query exists; the confidence adjustment does not.

2. **Provider disagreement flagging** (small effort) — When one provider extracts `A → acts_on → B` and the other extracts `A → contraindicated_with → B`, that's a contradiction worth flagging. Use `observations_for_edge()` to detect conflicting edge types between the same node pair. This is the seed of the contradiction detection reasoning mode.

3. **Consensus extraction mode** (medium effort) — Run both providers on the same question, extract from both, and only insert edges that appear in both results. Edges unique to one provider go into a candidate/quarantine table. Configurable: `--consensus` flag for strict mode, default for union mode.

---

## Priority Assessment

| Concern | Severity | Effort | When |
|---|---|---|---|
| ~~Terminal node flag~~ | ~~Medium~~ | ~~Trivial~~ | **Done** (2026-03-22) |
| ~~Source tracking layer~~ | ~~High~~ | ~~Medium~~ | **Done** (2026-03-22) |
| Gap-fill staleness | Medium (noise source) | Small | Now |
| Coverage metrics | Medium (ignorance) | Small | Now |
| Observation ranking | High (scaling) | Medium | Before adding more ingredients |
| Cross-provider intersection scoring | High (quality) | Small | Now (infrastructure in place) |
| Reasoning depth limits | Medium (cascade) | Small | Before adding more ingredients |
| Speculative edge quarantine | High (noise/signal) | Medium | Now (source layer supports it) |
| Supernode dampening | High (scaling) | Medium | Before 10+ ingredients |
| Pipeline contract tests | Medium (correctness) | Medium | With synonym resolution |
| Confidence decay | Medium (noise) | Medium | Now (source layer supports it) |
