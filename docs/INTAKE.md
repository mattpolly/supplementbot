# Intake System — Design & Implementation

*Intake agent design reviewed by Claude, Gemini, and Grok (2026-03-24). Intake KG reviewed by Gemini and Grok (2026-03-26). Full implementation complete and integrated (2026-03-27).*

---

## Overview

The intake system conducts a clinical supplement interview via WebSocket chat. It has two main pieces:

1. **Intake KG** — a process graph encoding *how* to interview a patient (what to ask, when, why)
2. **Intake Agent** — session state, safety filters, concept mapping, and the LLM context builder

The intake KG is the orchestrator. The LLM is a renderer — it never decides what to ask.

### Two Graphs, One System

| | Supplement KG | Intake KG |
|---|---|---|
| **Purpose** | Domain knowledge — what we know about supplements | Process knowledge — how to interview a patient |
| **Encodes** | Symptom → System → Mechanism → Ingredient | Question → Response Pattern → Next Action |
| **Traversal** | "What ingredients address muscle cramps?" | "What should I ask next given what I know?" |
| **Topology** | Ingredient-outward, multi-hop paths | Symptom-inward, decision-tree-like with cross-links |

The intake KG consults the supplement KG dynamically via `GraphAction` nodes. Adding a supplement to the supplement KG automatically creates new differentiation possibilities without touching the intake KG.

### Legal Constraints (non-negotiable)

- **Never diagnose.** Never say "you have X condition."
- **Never say "cure."** Supplements address symptoms, not diseases.
- **No direct `relieves` edge.** Symptom → Ingredient is always indirect through System/Mechanism nodes.
- All output framed as "supplements that act on systems where your symptoms present."
- Never recommend specific dosages.

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                  Web Frontend (chat UI)               │
└─────────────────────┬────────────────────────────────┘
                      │ WebSocket
┌─────────────────────▼────────────────────────────────┐
│                  Web Server (axum)                     │
│                                                       │
│  ┌─────────────────────────────────────────────────┐  │
│  │              Turn Pipeline (v2)                  │  │
│  │                                                 │  │
│  │  1. Red flag check                              │  │
│  │  2. Extract (cheap LLM)                         │  │
│  │  3. Apply extraction to session                 │  │
│  │  4. Concept map → SymptomProfile resolution     │  │
│  │  5. Build TraversalContext                       │  │
│  │  6. IntakeEngine::next_turn() → TurnAction      │  │
│  │  7. GraphActionExecutor::execute()              │  │
│  │  8. Update session (candidates, phase, lens)    │  │
│  │  9. build_context_v2() + renderer LLM           │  │
│  │ 10. Post-generation safety filter               │  │
│  └─────────────────────────────────────────────────┘  │
│         │                    │                         │
│  ┌──────▼──────┐   ┌────────▼────────┐                │
│  │ Intake KG   │   │ Supplement KG   │                │
│  │ (process)   │──▶│ (domain)        │                │
│  └─────────────┘   └─────────────────┘                │
└───────────────────────────────────────────────────────┘
```

---

## Intake Knowledge Graph

**Crate:** `graph-service` — `crates/graph-service/src/intake/`

Shares the same SurrealDB instance as the supplement KG with `intake_`-prefixed tables.

### The Problem It Solves

The original v1 intake had a hardcoded phase state machine and relied on prompt engineering. Results were poor:

- **Repeated questions** — LLM asks about alleviating factors three times
- **Irrelevant questions** — asks about radiation for GI symptoms
- **Poor transitions** — moves to recommendation too early or stays in HPI too long
- **No symptom-specific knowledge** — treats every intake identically

Root cause: the LLM was asked to reason about clinical interviewing with no structured knowledge of what a good intake looks like.

### Node Types

**Process nodes** (the interview structure):

| Type | Description |
|------|-------------|
| `IntakeStage` | A phase: chief_complaint, hpi, system_review, differentiation, causation_inquiry, recommendation |
| `QuestionTemplate` | Parameterized question with {placeholders} and optional OLDCARTS dimension target |
| `ClinicalGoal` | What info to gather (e.g., "characterize_onset"), linked to extractor fields |
| `ExitCondition` | Gate for leaving a stage (OldcartsSufficient, UserDisengaged, CandidatesConfident, etc.) |

**Domain-bridge nodes** (connect to supplement KG):

| Type | Description |
|------|-------------|
| `SymptomProfile` | Intake-specific symptom knowledge with UMLS CUI, iDISK aliases, archetype reference, associated systems |
| `ArchetypeProfile` | Category template (~10 archetypes: pain, sleep, mood, digestive, etc.) with relevant/irrelevant OLDCARTS dims |
| `SymptomCluster` | Co-occurring pattern (e.g., fatigue + cold intolerance → thyroid) with prioritized systems |
| `SystemReviewNode` | Body system with screening questions |
| `GraphActionNode` | Trigger for supplement KG queries (QueryCandidates, FindDiscriminators, CheckInteractions, etc.) |

### Edge Types (14)

HasStage, Asks, Fulfills, FallsBack, RelevantFor, IrrelevantFor, BelongsTo, CoOccurs, Suggests, Triggers, ExitsWhen, EscalatesTo, Probes, HasGoal

Each edge carries `IntakeEdgeMeta`: priority (0.0–1.0), required, safety_gate, max_asks, optional condition.

### Traversal: Expected Information Gain

Static priority alone produces robotic interviews. Edge selection uses EIG:

```
effective_priority = base_priority × information_gain × system_relevance
```

- **information_gain**: How many candidates does this question eliminate?
- **system_relevance**: Multiplier from active `SymptomProfile` (radiation = 0.0 for GI, 0.9 for chest pain)
- **base_priority**: Static weight from edge metadata, used as tiebreaker

The engine is stateless — takes a `TraversalContext` snapshot and returns a `TurnAction` (question, graph actions, stage transition, debug trace).

### GraphAction Executor

Bridge between the two graphs. When the engine says "do X", the executor runs X against the supplement KG:

| Action | What it does |
|--------|--------------|
| `QueryCandidates` | Pattern-based traversal (DirectSystem, ViaMechanism) with lens filtering |
| `FindDiscriminators` | Walk outgoing edges from candidates, find non-shared systems |
| `CheckInteractions` | Match disclosed medications against iDISK `interacts_with` edges |
| `CheckAdverseReactions` | Cross-reference symptoms against iDISK `has_adverse_reaction` edges |
| `FetchMechanism` | Pull Mechanism of Action text from iDISK for recommendation framing |
| `FindAdjacentSystems` | Discover systems connected to candidates for ROS |

### Data Sources

**iDISK 2.0** (`IDISK_DATA_DIR` env var, loaded at startup if present):
- 392 symptoms (SS.csv) → `SymptomProfile` nodes with MSKCC aliases + UMLS CUIs
- 7,876 ingredients (DSI.csv) → mechanism of action, safety text
- 214 drugs (D.csv) → interaction checking
- 1,274 adverse reaction edges (dsi_ss.csv)
- 536 drug interaction edges with descriptions (dsi_d.csv)

**SuppKG**: Lookup-only oracle for validation (never import edges directly).

### Population Strategy

1. **Core structure** — 6 stages, goals, questions, exit conditions, edges (in `seed.rs`, idempotent)
2. **Archetype profiles** (~10) + symptom profiles (392 from iDISK) with archetype assignment
3. **Drug & interaction knowledge** from iDISK relation files
4. **Ingredient enrichment** — mechanism of action, safety text from iDISK DSI entities
5. **System reviews** + dynamic GraphAction integration

---

## Intake Agent

**Crate:** `intake-agent` — `crates/intake-agent/src/`

### Session State (`session.rs`)

Tracks a single conversation. Key fields:

- `phase: IntakePhase` — six phases (ChiefComplaint → Hpi → ReviewOfSystems → Differentiation → CausationInquiry → Recommendation)
- `chief_complaints: Vec<ChiefComplaint>` — raw text + mapped symptoms/systems
- `oldcarts: OldcartsState` — 9 clinical dimensions with `filled_dimensions()` → `HashSet<OldcartsDimension>`
- `candidates: CandidateSet` — ranked ingredient candidates
- `lens_level: f64` — complexity dial, escalates as detail accumulates
- `visited_questions: HashSet<String>` — prevents the engine from re-asking
- `goal_ask_counts: HashMap<String, u8>` — respects `max_asks` on edges
- `active_profiles: Vec<String>` — SymptomProfile IDs driving archetype-aware question selection
- `disclosed_supplements: Vec<String>` — for adverse reaction checking
- `last_differentiator_count: usize` — from previous turn's executor (engine needs it for transitions)

### Phase Transitions

The engine evaluates exit conditions per stage:
- **CC → HPI**: when at least one chief complaint is recorded
- **HPI → SystemReview/Differentiation/Recommendation**: when OLDCARTS dimensions are sufficient for active profiles, confidence is high enough, or user signals done
- **SystemReview → Differentiation/Recommendation**: when all cluster-prioritized systems reviewed
- **Differentiation → CausationInquiry**: when disclosed medications have interactions to discuss
- **CausationInquiry → Recommendation**: always (single-turn acknowledgment)
- **Medication safety gate**: cannot reach Recommendation without asking about medications

### Safety (Three Layers)

1. **Red flag ejector** — pattern matches ~20 emergency keywords (chest pain, suicidal, etc.); blocks normal flow with static emergency UI
2. **Prompt constraints** — system prompt enforces "research suggests..." framing, never prescriptive
3. **Post-generation filter** — regex blacklist catches diagnosis patterns, "cure", dosage prescriptions. Returns Pass/Rewrite/Block.

### Concept Mapping (`concept_map.rs`)

Three-tier hybrid (v1 implements tier 1):
1. **Exact/alias match** — string match + merge table lookup (~40% of cases)
2. **Embedding similarity** — SurrealDB vector indexes (future)
3. **LLM ranker** — fallback for ambiguous matches (future)

After mapping to supplement KG nodes, the handler resolves to intake KG `SymptomProfile` IDs via `intake_store.find_symptom_profile()`.

### Context Generator (`context.rs`)

Two implementations:

- **`build_context` (v1)** — hardcoded OLDCARTS mnemonic, all 9 dimensions, generic per-phase task instructions
- **`build_context_v2` (v2, active)** — graph-driven: only relevant OLDCARTS dimensions for active profiles, task instruction from engine's `TurnAction` output, iDISK interaction/adverse/mechanism data from executor results

### Candidate Scoring

The executor's `QueryCandidates` action runs `QueryEngine::ingredients_for_symptom()` and returns `CandidateResult` (ingredient, score, quality, path explanations). The handler converts these to a thin `CandidateSet` for the session — `build_context_v2` only reads names and composite scores.

The old `candidates::score_candidates()` with intersection gate + coverage bonus still exists (used in tests) but the handler no longer calls it.

### Differentiators

The executor's `FindDiscriminators` action walks outgoing edges from candidates in the supplement KG to find non-shared systems. Results are entropy-scored (1.0 at perfect 50/50 split, 0.0 at 100/0).

The old `differentiator::compute_differentiators()` with depth-aware walks still exists but is replaced by the executor.

---

## Turn-by-Turn Walkthrough

Patient: "My legs hurt at night and I can't sleep."

**Turn 1 — Chief Complaint**
1. Engine at `IntakeStage:chief_complaint`, selects "what_brings_you_in" question
2. Extractor identifies symptoms: "leg pain", "insomnia"
3. Concept map resolves to `SymptomProfile:leg_pain` (pain archetype) + `SymptomProfile:insomnia` (sleep archetype)
4. `GraphAction:QueryCandidates` fires → supplement KG returns initial candidates
5. `ExitCondition:has_chief_complaint` met → transition to HPI

**Turns 2–5 — HPI (EIG-driven OLDCARTS)**
1. Pain archetype marks Location and Radiation as irrelevant for leg pain → skipped
2. EIG selects onset (high priority + high information gain) → "When did the leg pain start?"
3. Patient: "Two weeks ago" → extractor fills onset → goal fulfilled
4. `SymptomCluster` check: leg pain + insomnia matches "electrolyte_deficiency" cluster → prioritizes nervous system review
5. After sufficient dimensions filled → transition to SystemReview

**Turns 6–7 — System Review**
1. `GraphAction:FindAdjacentSystems` identifies systems connected to candidates
2. Cluster-prioritized systems asked first (nervous, then musculoskeletal)
3. Denials recorded as pertinent negatives → penalize dependent candidates

**Turn 8 — Differentiation**
1. `GraphAction:FindDiscriminators` finds non-shared nodes between top candidates
2. Entropy-sorted: prefer questions that split candidates closest to 50/50

**Turn 9 — Medication Check (safety gate)**
1. `ClinicalGoal:medication_check` is `safety_gate: true` — cannot skip
2. "Are you currently taking any prescription medications or other supplements?"
3. Patient: "I take magnesium glycinate and an SSRI"

**Turn 9b — Causation Inquiry (if applicable)**
1. `GraphAction:CheckInteractions` → SSRI found in iDISK interaction edges
2. `GraphAction:CheckAdverseReactions` → cross-reference symptoms against disclosed supplements
3. "I want to mention — some supplements can interact with SSRIs. Let me factor that into what I share."

**Turn 10 — Recommendation**
1. `GraphAction:FetchMechanism` pulls Mechanism of Action text from iDISK
2. LLM renders using sourced text: "For symptoms like yours, the literature suggests..."
3. Interaction warnings included with source descriptions from iDISK
4. "Please discuss these with your healthcare provider."

---

## Decisions (Three-Model Consensus)

| Question | Decision | Who |
|----------|----------|-----|
| Clinical workflow | OLDCARTS → ROS → Differentiation → Recommendation | All three |
| Pertinent negatives | `systems_denied` with score penalty | Gemini (proposed), all agreed |
| Associated symptoms | Tracked per CC, can shift system mapping | Gemini |
| Differentiator depth | Walk deeper until divergence found | Gemini + Grok |
| Safety layers | Three-layer: red flag → prompt → post-gen filter | All three |
| UI disclaimer | Injected at UI level, not LLM-generated | Gemini + Grok |
| Concept mapping | Hybrid: exact/alias → embedding → LLM ranker | All three |
| LLM role | Pure renderer — no graph access, full traceability | All three |
| Candidate scoring | Re-run QueryEngine each turn via executor | Claude + Grok |
| Multi-symptom scoring | Intersection gate + sum + coverage bonus | Grok |
| Intake KG traversal | EIG, not static priority | Gemini |
| Storage | Same SurrealDB, `intake_`-prefixed tables | Grok |
| ResponsePattern | Demoted — extractor fields map directly to `fulfills` edges | Grok |
| GraphAction | Keep as node (not edge property) — inspectability wins | Grok over Gemini |
| `max_asks` on edges | Caps repeated probing per goal (default 2) | Grok |
| Safety gates | Engine-level enforcement, non-bypassable | Grok |
| SymptomCluster | New node type for co-occurring patterns | Gemini |
| CausationInquiry stage | Adverse reaction / de-prescribing checks | Gemini |
| Archetype Profiles | ~10 archetypes covering 80% of interview logic | Gemini |
| Context management | Summarize turns after ~8 | Grok |

---

## Implementation Notes (2026-03-27)

### Key decisions made during build

**Phase ↔ Stage mapping.** `IntakeSession` uses `IntakePhase` (intake-agent crate) while the engine uses `IntakeStageId` (graph-service crate). The handler converts with `phase_to_stage()` / `stage_to_phase()`.

**Differentiator count is from the previous turn.** The engine needs it for transitions, but discriminators come from the executor which runs after the engine. Solution: `last_differentiator_count` persisted on the session.

**CandidateResult → CandidateSet conversion is thin.** Empty `per_symptom_scores` and `supporting_paths` — `build_context_v2` only reads names and scores.

**Symptom → SymptomProfile resolution.** After concept mapping to supplement KG nodes, `intake_store.find_symptom_profile()` resolves to intake KG profile IDs.

**Disclosed supplements vs medications.** Handler currently treats all disclosed items as both. Future: split based on iDISK ingredient table lookup.

**Relevant dimensions passed externally.** `IntakeEngine::relevant_dimensions()` made public so handler can pass to `build_context_v2()`.

### What v2 replaced

| Step | v1 (hardcoded) | v2 (intake KG) |
|------|----------------|----------------|
| Candidate scoring | `candidates::score_candidates()` | `GraphActionExecutor::QueryCandidates` |
| Differentiators | `differentiator::compute_differentiators()` | `GraphActionExecutor::FindDiscriminators` |
| Phase transitions | `phase::evaluate_transition()` | `IntakeEngine::next_turn()` |
| Question selection | Hardcoded per-phase prompts | Engine EIG scoring |
| Context building | `build_context()` with all 9 OLDCARTS dims | `build_context_v2()` with relevant dims only |
| Drug interactions | Not checked | `CheckInteractions` via iDISK |
| Adverse reactions | Not checked | `CheckAdverseReactions` via iDISK |
| Mechanism text | Not available | `FetchMechanism` via iDISK |

Old modules (`phase.rs`, `differentiator.rs`, `candidates.rs`) still compile and are used in tests but the handler no longer calls them.

---

## Open Questions

### Resolved
1. ✅ SymptomProfile granularity: hierarchical (Archetype → Profile) *(Gemini)*
2. ✅ Traversal engine: EIG *(Gemini)*
3. ✅ Universal vs symptom-specific: ~80/20 *(Gemini)*
4. ✅ CUI bridging: mandatory *(Gemini)*
5. ✅ Storage: same SurrealDB instance *(Grok)*
6. ✅ ResponsePattern: reuse extractor directly *(Grok)*
7. ✅ Implicit fulfillment: post-extraction graph check *(Grok)*
8. ✅ GraphAction: keep as node *(Grok over Gemini)*

### Open
9. **Versioning** — tag intake KG nodes with `min_supplement_kg_version` or use CUI-based references?
10. **Adverse reaction legal framing** — how far can we go? "Discuss with your doctor" seems safe.
11. **Product-level knowledge** — resolve brand names to iDISK ingredients? Overkill for v1?
12. **iDISK Source_Description** — feed interaction descriptions directly into prompt, or summarize?
13. **SymptomCluster population** — how many clusters needed? LLM-assisted discovery + human review.
14. **Multi-symptom parallel profiles** — goal merging across active profiles. ✅ Implemented.
15. **Safety bypass prevention** — engine-level enforcement. ✅ Implemented via `safety_gate` on edges.
16. **`max_asks` default** — set to 2. Working well.
17. **Debugging infrastructure** — synthetic patient harness and coverage metrics still needed.
18. **Hardcoded task removal** — ✅ Done. Graph traversal output is the task instruction.

---

## Multi-Model Review Log

### Gemini (2026-03-26)

| Recommendation | Status |
|---|---|
| `SymptomCluster` node type | ✅ Adopted |
| Expected Information Gain traversal | ✅ Adopted |
| CUI mapping mandatory | ✅ Adopted |
| Archetype Profiles | ✅ Adopted |
| Fallback edges for silent stalls | ✅ Adopted |
| `visited_nodes` registry | ✅ Adopted |
| `CausationInquiry` stage | ✅ Adopted |

**Key insight**: "Most intake systems only look for solutions. By using iDISK's adverse reaction data, your Intake KG can perform a de-prescribing check."

### Grok (2026-03-26)

| Recommendation | Status |
|---|---|
| Same SurrealDB instance | ✅ Adopted |
| Extractor schema as single source of truth | ✅ Adopted |
| Implicit fulfillment as post-extraction check | ✅ Adopted |
| Keep `GraphAction` as node | ✅ Adopted (overrode Gemini) |
| `max_asks` on edge metadata | ✅ Adopted |
| Multi-symptom parallel profiles | ✅ Adopted |
| Non-bypassable safety gates | ✅ Adopted |
| Remove hardcoded task instructions | ✅ Adopted |

**Key insight**: "ResponsePattern as a first-class node adds indirection without value — extractor fields map directly to `fulfills` edges."

**Key insight**: "Safety must be structural, not advisory. Make `required: true` + SafetyGate non-ignorable at the engine level."

**Divergence from Gemini**: Gemini suggested `GraphAction` as edge property; Grok says keep as node. Grok's argument (inspectability, extensibility) won.

---

## Future Work

- **User accounts / authentication** — single-user prototype first
- **Persistent session history** — in-memory for v1
- **Dosage recommendations** — out of scope, additional legal complexity
- **Embedding-based concept mapping** — tier 2 of the hybrid approach
- **Supplement vs medication splitting** — use iDISK ingredient table to classify disclosed items
- **Synthetic patient test harness** — regression testing for intake quality
- **"Why this question?" transparency** — expandable cards showing graph basis
- **Dual-graph alignment (Reflection Modeling)** — long-term: patient session state as a temporal KG, with embedding similarity + graph traversal overlaps producing "reflection bridges"
