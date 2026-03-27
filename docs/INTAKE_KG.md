# Intake Knowledge Graph — Design Document

*Draft — 2026-03-26. Multi-model reviewed (Gemini, Grok). Core implementation complete (2026-03-27) — types, store, seed, engine, executor, iDISK importer all built. Integration into web-server handler pending.*

## The Problem

The current intake agent has a hardcoded phase state machine (ChiefComplaint → HPI → ROS → Differentiation → Recommendation) and relies on prompt engineering to guide the LLM through clinical interviewing. The results are poor:

- **Repeated questions** — the LLM asks about alleviating factors three times because the extractor doesn't reliably fill OLDCARTS slots, and the prompt just says "(not yet asked)"
- **Irrelevant questions** — asks about radiation for GI symptoms, asks OLDCARTS dimensions that don't apply to the complaint
- **Poor transitions** — moves to recommendation too early, stays in HPI too long, doesn't know when enough is enough
- **No symptom-specific knowledge** — treats every intake identically regardless of what the patient describes
- **Off-topic fragility** — off-topic questions get misclassified, derailing the session

Root cause: **the LLM is being asked to reason about clinical interviewing, but it has no structured knowledge of what a good intake looks like.** We gave it a mnemonic and some phase labels and hoped it would figure out the rest. It won't.

## The Solution

An **Intake Knowledge Graph** — a process graph that encodes how to conduct a clinical supplement intake. The LLM becomes purely a natural language renderer; the graph drives all reasoning about what to ask, when, and why.

This is NOT a second disconnected graph. The intake KG **consults** the existing supplement KG as part of its process. Some intake steps involve querying the supplement graph for domain-specific content (discriminating questions, candidate scoring, system adjacency).

### Two Graphs, One System

| | Supplement KG (exists) | Intake KG (new) |
|---|---|---|
| **Purpose** | Domain knowledge — what we know about supplements | Process knowledge — how to interview a patient |
| **Encodes** | Symptom → System → Mechanism → Ingredient | Question → Response Pattern → Next Action |
| **Traversal** | "What ingredients address muscle cramps?" | "What should I ask next given what I know?" |
| **Changes** | Grows as we add supplements/research | Relatively stable once encoded |
| **Topology** | Ingredient-outward, multi-hop paths | Symptom-inward, decision-tree-like with cross-links |

The intake KG is the **orchestrator**. Instead of a hardcoded state machine, the system walks the intake graph to decide what to do next. Some of those steps involve walking the supplement graph.

---

## Proposed Schema

### Node Types

#### Process Nodes (the interview structure)

| Type | Description | Example |
|------|-------------|---------|
| `IntakeStage` | A phase of the interview | "chief_complaint", "hpi", "system_review", "differentiation", "causation_inquiry", "recommendation" |
| `QuestionTemplate` | A parameterized question the agent can ask | "When did {symptom} start?", "Have you noticed any {system} changes?" |
| `ClinicalGoal` | What information we're trying to gather | "characterize_symptom", "identify_system_involvement", "check_medications" |
| `ExitCondition` | When to leave a stage | "oldcarts_sufficient", "user_disengaged", "candidates_confident" |

*Note: `ResponsePattern` was demoted from a first-class node type (Grok review). The extractor already produces structured fields (OLDCARTS slots, symptom mentions, engagement signals). `fulfills` edges map directly from extractor output fields to `ClinicalGoal` nodes. No separate classification layer needed.*

#### Domain-Bridge Nodes (connect to supplement KG)

| Type | Description | Example |
|------|-------------|---------|
| `SymptomProfile` | Intake-specific knowledge about a symptom or symptom category | "muscle_cramps", "insomnia", "fatigue" |
| `SymptomCluster` | A co-occurring symptom pattern that points to a common underlying cause | "fatigue + cold_intolerance + hair_thinning → thyroid_pattern" |
| `ArchetypeProfile` | A category template that groups symptom profiles sharing similar interview logic | "pain_archetype", "sleep_archetype", "mood_archetype" |
| `SystemReview` | What to ask when probing a body system | "digestive_review", "nervous_review" |
| `GraphAction` | A step that queries the supplement KG | "query_candidates", "find_discriminators", "check_contraindications", "check_adverse_reactions" |

### Edge Types

| Edge | From → To | Description |
|------|-----------|-------------|
| `has_stage` | IntakeStage → IntakeStage | Stage ordering (with conditions) |
| `asks` | ClinicalGoal → QuestionTemplate | What questions serve this goal |
| `fulfills` | ExtractorField → ClinicalGoal | When an extractor output field satisfies a goal (e.g., `onset` field → `ClinicalGoal:characterize_onset`) |
| `falls_back` | QuestionTemplate → QuestionTemplate | Fallback if response doesn't match any pattern (rephrase/clarify) |
| `relevant_for` / `irrelevant_for` | QuestionTemplate → SymptomProfile | Which questions apply/don't apply to which symptoms |
| `belongs_to` | SymptomProfile → ArchetypeProfile | Inherits default interview logic from archetype |
| `co_occurs` | SymptomProfile → SymptomCluster | Symptom participates in a known co-occurrence pattern |
| `suggests` | SymptomCluster → SystemReview | Cluster pattern prioritizes specific system reviews |
| `triggers` | ExtractorField → GraphAction | When an extraction result should trigger a supplement KG query (e.g., medications extracted → check_interactions) |
| `exits_when` | IntakeStage → ExitCondition | Conditions for leaving a stage |
| `escalates_to` | ExitCondition → IntakeStage | Where to go when an exit condition is met |
| `probes` | SystemReview → QuestionTemplate | What questions to ask when reviewing a system |

### Edge Metadata

```rust
pub struct IntakeEdgeMetadata {
    pub priority: f64,              // 0.0–1.0, base priority for traversal ordering
    pub required: bool,             // must this edge be traversed, or is it optional?
    pub safety_gate: bool,          // if true, engine CANNOT skip — non-bypassable
    pub max_asks: u8,               // max times to probe this goal before moving on (default: 2)
    pub condition: Option<String>,  // e.g., "only if candidates > 0"
}
```

### Traversal Strategy: Expected Information Gain

Static priority alone produces robotic interviews. Instead, edge selection uses **Expected Information Gain (EIG)**:

```
effective_priority = base_priority × information_gain × system_relevance
```

- **information_gain**: How many candidates does this question eliminate? If Question A narrows 10 candidates to 2 and Question B narrows 10 to 8, pick A even if B has higher base priority.
- **system_relevance**: Multiplier from the active `SymptomProfile`. Radiation gets 0.0 for GI symptoms, 0.9 for chest pain. Prevents irrelevant questions without hard rules.
- **base_priority**: The static weight from edge metadata, used as tiebreaker.

The entropy scoring we already compute in the differentiator extends naturally to this — the differentiator finds which supplement KG nodes discriminate between candidates, and EIG selects which intake KG question to ask about those nodes.

### Session State: Visited Nodes

The traversal engine maintains a `visited_nodes: HashSet<NodeId>` in session state. Before traversing any edge, the engine checks whether the target `QuestionTemplate` has already been visited. This prevents the repeated-question problem at the structural level — the graph literally cannot revisit a question node within the same session, even if multiple traversal paths lead to it.

---

## How It Works: Walkthrough

### Turn-by-Turn Example

Patient says: "My legs hurt at night and I can't sleep."

**Turn 1 — Chief Complaint**

1. System is at `IntakeStage:chief_complaint`
2. Intake KG receives the raw text
3. Traverse: `chief_complaint -[asks]→ QuestionTemplate:"what_brings_you_in"`
4. Patient responds. Extractor identifies symptoms: "leg pain", "insomnia"
5. Intake KG looks up `SymptomProfile:leg_pain` and `SymptomProfile:insomnia`
6. `GraphAction:query_candidates` fires → supplement KG returns initial candidates
7. `ExitCondition:has_chief_complaint` is met → `escalates_to` → `IntakeStage:hpi`

**Turn 2–5 — HPI (OLDCARTS), now driven by EIG**

1. System is at `IntakeStage:hpi`
2. For `SymptomProfile:leg_pain` (inherits from `ArchetypeProfile:pain`), intake KG knows:
   - `relevant_for`: onset, location, character, timing, severity, aggravating
   - `irrelevant_for`: radiation (leg pain doesn't radiate like chest pain; system_relevance = 0.0)
3. EIG selects next question: onset has high base_priority AND high information_gain (timing narrows candidates significantly) → "When did the leg pain start?"
4. Patient: "About two weeks ago." → extractor fills `onset` field → `fulfills` → `ClinicalGoal:characterize_onset`
5. Next highest EIG: location (but already implied — "legs"). `visited_nodes` check: location already fulfilled from chief complaint (implicit fulfillment)
6. Skip to character: "What does it feel like?"
7. Patient gives vague answer → `falls_back` edge fires → rephrase: "Is it more of an ache, or more like cramping?"
8. `ExitCondition:oldcarts_sufficient` checks: enough dimensions filled for this symptom profile? `SymptomProfile:leg_pain` says 4 dimensions is sufficient (not all 9 OLDCARTS apply equally)
9. Meanwhile, `SymptomCluster` check: leg pain + insomnia `co_occurs` in "electrolyte_deficiency" cluster → `suggests` → prioritize `SystemReview:nervous` over `SystemReview:digestive`
10. When sufficient → `escalates_to` → `IntakeStage:system_review`

**Turn 6–7 — System Review**

1. `GraphAction:find_adjacent_systems` fires → supplement KG returns systems connected to current candidates
2. For each unreviewed system, intake KG has `SystemReview` nodes with specific questions
3. `SystemReview:nervous` → "Have you noticed any tingling or numbness?"
4. Patient denies → pertinent negative recorded
5. `SystemReview:digestive` → "Any changes in digestion?"
6. `ExitCondition:systems_reviewed` met → next stage

**Turn 8 — Differentiation**

1. `GraphAction:find_discriminators` fires → supplement KG identifies differentiating properties between top candidates
2. Intake KG imports these as temporary `QuestionTemplate` nodes
3. Questions sorted by entropy score (from supplement KG analysis)
4. "Do your symptoms get worse with exercise or better?" — discriminates between magnesium (worse with exercise → electrolyte) and calcium (worse at rest → deficiency)

**Turn 9 — Medication Check**

1. `ClinicalGoal:medication_check` is marked `required: true`
2. Cannot reach `IntakeStage:recommendation` without fulfilling it
3. "Are you currently taking any prescription medications or other supplements?"
4. Patient: "I take magnesium glycinate and an SSRI"

**Turn 9b — Causation Inquiry (conditional)**

1. `GraphAction:check_adverse_reactions` fires → iDISK's `has_adverse_reaction` edges cross-referenced against disclosed supplements and reported symptoms
2. Match found: magnesium at high doses `has_adverse_reaction` to GI symptoms (if relevant to this session)
3. No match for leg pain + magnesium in this case → skip causation inquiry
4. But: SSRI found → `GraphAction:check_interactions` fires → iDISK `interacts_with` edges checked against candidates
5. If interaction found: flag the candidate, route to `IntakeStage:causation_inquiry` to discuss before recommending
6. `IntakeStage:causation_inquiry`: "I want to mention — some supplements can interact with SSRIs. Let me factor that into what I share with you."

**Turn 10 — Recommendation**

1. `GraphAction:final_scoring` fires → supplement KG computes final candidate scores, interaction-flagged candidates downranked or excluded
2. `GraphAction:fetch_mechanism` pulls Mechanism of Action text from iDISK DSI entities for top candidates
3. Intake KG provides `QuestionTemplate` for recommendation framing
4. LLM renders using real sourced mechanism text: "For symptoms like yours, the literature suggests..."
5. Interaction warnings included where relevant, with source descriptions from iDISK

### Key Insight: The Supplement KG Consultation

At steps like "find_discriminators," the intake KG doesn't contain the discriminating questions statically. Instead, `GraphAction:find_discriminators` triggers a traversal of the supplement KG:

1. Get current candidate ingredients
2. For each pair, walk the supplement KG to find non-shared nodes (systems, mechanisms, properties)
3. Package the divergence points as question topics
4. Return them to the intake KG as traversal options

This means the intake KG stays lean (process only) while the supplement KG provides domain richness. Adding a new supplement to the supplement KG automatically creates new differentiation possibilities without touching the intake KG.

---

## What Changes in the Current System

### Replaces
- `phase.rs` state machine → graph traversal of `IntakeStage` nodes
- Generic OLDCARTS mnemonic in prompt → `SymptomProfile`-specific relevant dimensions
- Hardcoded `detect_signal()` heuristics → `ResponsePattern` matching
- Hardcoded `evaluate_transition()` → `ExitCondition` traversal

### Keeps
- `extract.rs` — still need LLM extraction of structured data from user text
- `context.rs` — still generates the LLM prompt, but now populated from graph traversal instead of hardcoded strings
- `candidates.rs` — scoring logic stays, but triggered by `GraphAction` nodes
- `safety.rs` — red flag ejector and post-gen filter are independent of the intake graph
- `session.rs` — session state still tracks the conversation, but phase management moves to graph

### New
- `intake-graph` crate or module — the intake KG schema, loader, and traversal engine
- Intake KG data files — the actual encoded interview knowledge (TOML/JSON/YAML)
- `GraphAction` executor — dispatches supplement KG queries from intake KG traversal

---

## Available Data Sources

### iDISK 2.0 (Integrated Dietary Supplement Knowledge Base)

We have the full iDISK 2.0 dataset at `data/idisk2/`. This is a curated, multi-source knowledge base from academic research.

#### Entities (`data/idisk2/Entity/`)

| File | Type | Count | Key Fields |
|------|------|-------|------------|
| `SS.csv` | Signs/Symptoms | 392 | iDISK_ID, Name, CUI, MSKCC aliases |
| `DSI.csv` | Dietary Supplement Ingredients | 7,876 | iDISK_ID, Name, CUI, **Background**, **Safety**, **Mechanism of Action**, Source Material |
| `D.csv` | Drugs | 214 | iDISK_ID, Name, CUI, MSKCC aliases |
| `Dis.csv` | Diseases | 172 | iDISK_ID, Name, CUI, MSKCC aliases |
| `DSP.csv` | Dietary Supplement Products | 163,806 | iDISK_ID, Name, Company, Purpose, Risk |

#### Relations (`data/idisk2/Relation/`)

| File | Relation | From → To | Count | Key Fields |
|------|----------|-----------|-------|------------|
| `dsi_ss.csv` | `has_adverse_reaction` | Ingredient → Symptom | 1,274 | Source |
| `dsi_dis.csv` | `is_effective_for` | Ingredient → Disease | 828 | Source, Effectiveness Rating |
| `dsi_d.csv` | `interacts_with` | Ingredient → Drug | 536 | Source, Interaction Rating, **Source_Description** |
| `dsp_dsi.csv` | `has_ingredient` | Product → Ingredient | 317,062 | — |

#### What iDISK Gives Us for the Intake KG

**1. Drug interaction knowledge (536 edges with descriptions)**

The `interacts_with` relation includes free-text Source_Description explaining the interaction mechanism. Example:

> DSI001441 (5-HTP) interacts_with D000000 (Antidepressants): "Because 5-HTP can also raise serotonin levels, there is the theoretical potential for increased risk of side effects or toxicities."

This directly feeds the **medication safety gate**. When the user says "I take antidepressants," we can:
- Look up all `interacts_with` edges for current candidate ingredients
- Flag contraindicated candidates with the actual reason
- Route the intake conversation to probe further if an interaction is found

**2. Adverse reaction mapping (1,274 edges)**

`has_adverse_reaction` tells us which ingredients **cause** symptoms. This enables a critical intake reasoning path:

> Patient: "I've been having mania-like episodes"
> System: 5-HTP `has_adverse_reaction` Mania → if patient is taking 5-HTP, this changes the whole conversation

The intake KG can include a `GraphAction:check_adverse_reactions` step that cross-references the user's reported symptoms against known adverse reactions of anything they're already taking. This is a different reasoning path than "what supplement helps this symptom" — it's "is a supplement *causing* this symptom."

**3. Mechanism of Action text on DSI entities**

Each ingredient has a `Mechanism of action` field with sourced, detailed mechanistic reasoning. Example (Selenium):

> "Selenium is an essential structural element of the antioxidant enzyme glutathione-peroxidase that converts aggressive oxidation products and intracellular free radicals into less reactive or neutral components."

This replaces LLM-fabricated explanations in the recommendation phase. The intake KG can include a `GraphAction:fetch_mechanism` that pulls this text and feeds it to the renderer so it can explain *why* a supplement is relevant using real sourced content.

**4. Safety text on DSI entities**

The `Safety` field on ingredients contains warnings, contraindication details, and population-specific cautions. This feeds the safety filter and the recommendation framing.

**5. MSKCC alias expansion on SS entities**

Each symptom has aliases from MSKCC. Example:

> SS000002 (Pain): "Pain | pain | irritating pain | PAIN EYE | Pain, NOS | Application site pain | absence of pain sensation"

This massively improves concept mapping during extraction. The current system struggles to map user language to ontology terms. iDISK's alias lists provide a ready-made synonym table for 392 symptoms.

**6. Disease-level concepts (172 entities, 828 effectiveness edges)**

Our current supplement KG works at the symptom level. iDISK adds disease-level nodes with `is_effective_for` edges. This bridges the gap — symptoms cluster into conditions, and some supplement evidence is condition-level, not symptom-level. Example: "5-HTP is_effective_for Depression" is a stronger signal than matching individual depressive symptoms.

### SuppKG (existing)

Already loaded. `data/supp_kg.json` and `data/suppkg_v2.edgelist`. Lookup-only — we never import its edges directly, but use it as an oracle for validation and gap-filling.

### Cross-Source Integration

iDISK and SuppKG overlap but don't duplicate. Both use UMLS CUIs, which gives us a natural join key. The integration strategy:

1. **Symptoms**: iDISK SS entities (392) become the canonical symptom vocabulary, with MSKCC aliases feeding concept mapping
2. **Ingredients**: iDISK DSI entities (7,876) enrich existing supplement KG nodes with Background, Safety, Mechanism of Action
3. **Drug interactions**: New edge type, sourced entirely from iDISK `interacts_with`
4. **Adverse reactions**: New edge type, sourced from iDISK `has_adverse_reaction`
5. **Products**: iDISK DSP entities (163,806) provide brand-name → ingredient mapping for when users mention specific products

---

## Population Strategy

The intake KG needs to be populated with clinical interview knowledge. This is the tedious but crucial part — but iDISK gives us a significant head start.

### Phase 1: Core Structure
- Encode the six `IntakeStage` nodes (chief_complaint, hpi, system_review, differentiation, causation_inquiry, recommendation) and their transitions
- Encode `ClinicalGoal` nodes for each OLDCARTS dimension + medication check + system review
- Create `ExitCondition` nodes with concrete criteria
- Create `falls_back` edges on every `QuestionTemplate` for clarification/rephrasing when response doesn't match any `ResponsePattern`

### Phase 2: Archetype Profiles + Symptom Profiles (bootstrapped from iDISK)

**Step 1 — Define ~10 Archetype Profiles** (the 80% that's universal per category):

| Archetype | Relevant OLDCARTS | Sufficient | Primary Systems |
|-----------|-------------------|------------|-----------------|
| `pain` | onset, location, character, severity, aggravating, timing | 4 | musculoskeletal, nervous, vascular |
| `sleep` | onset, duration, timing, aggravating, alleviating | 3 | nervous, endocrine |
| `mood` | onset, duration, character, severity, timing | 3 | nervous, endocrine |
| `digestive` | onset, location, character, timing, aggravating | 4 | digestive, immune |
| `fatigue` | onset, duration, severity, timing, aggravating | 3 | endocrine, immune, nervous |
| `skin` | onset, location, character, aggravating | 3 | immune, integumentary |
| `respiratory` | onset, character, timing, aggravating, severity | 3 | respiratory, immune |
| `cardiovascular` | onset, character, timing, severity, radiation | 4 | cardiovascular, nervous |
| `immune` | onset, duration, timing, severity | 3 | immune, lymphatic |
| `cognitive` | onset, duration, character, timing, severity | 3 | nervous, vascular |

**Step 2 — Import iDISK SS entities (392 symptoms) as `SymptomProfile` nodes.** Each gets:
- MSKCC aliases for concept mapping
- `belongs_to` edge linking to its `ArchetypeProfile` (inherits default interview logic)
- iDISK CUI for cross-referencing

**Step 3 — LLM-assisted archetype assignment.** Use a judge LLM to map each of the 392 iDISK symptoms to its best-fit archetype. Human review the assignments.

**Step 4 — Manual tuning for top ~20 high-frequency symptoms.** These override archetype defaults with symptom-specific interview logic (custom relevant_oldcarts, custom sufficient_dimensions, custom response patterns). The top 20 likely cover 80% of real traffic.

**Step 5 — SymptomCluster nodes.** Encode known co-occurrence patterns:
- fatigue + cold_intolerance + hair_thinning → thyroid_pattern → prioritize endocrine review
- muscle_cramps + insomnia → electrolyte_pattern → prioritize nervous review
- mood_changes + fatigue + digestive_issues → gut_brain_pattern → prioritize both systems

Use iDISK `has_adverse_reaction` edges to flag symptoms that might be supplement-caused (enables causation inquiry path).

### Phase 3: Drug & Interaction Knowledge (from iDISK)
- Import iDISK Drug entities (214 drugs) as interaction-check nodes
- Import `interacts_with` edges (536) with Source_Description text
- Wire into `ClinicalGoal:medication_check` — when user reports medications, traverse interaction edges against current candidates
- Import `has_adverse_reaction` edges (1,274) for reverse-causation reasoning

### Phase 4: Ingredient Enrichment (from iDISK)
- Attach Mechanism of Action, Safety, and Background text from DSI entities to supplement KG ingredient nodes
- These feed the recommendation phase — real sourced explanations instead of LLM fabrication
- Import `is_effective_for` disease-level edges (828) to supplement symptom-level evidence

### Phase 5: System Reviews
- For each body system in the supplement KG:
  - Plain-language screening questions
  - Common positive/negative response patterns
  - Priority relative to current candidates

### Phase 6: Dynamic Integration
- `GraphAction` nodes that query the supplement KG for:
  - Candidate scoring
  - Discriminating questions (diff walks that find non-shared nodes between candidates)
  - Contraindication checks (cross-reference candidates against `interacts_with` edges)
  - Adverse reaction checks (cross-reference symptoms against `has_adverse_reaction` edges)
  - System adjacency discovery
  - Mechanism of Action retrieval for recommendation framing

---

## Current Intake Prompt (for reference)

The current system rebuilds this prompt every turn from `context.rs`. This is what the intake KG replaces the reasoning portions of — the LLM still renders, but the graph provides the content.

```
ROLE: Supplement intake specialist

COMMUNICATION STYLE:
  - Short responses, one question per turn, plain language, no markdown/emoji

LEGAL CONSTRAINTS:
  - Never diagnose, never say "cure", never prescribe
  - Report findings: "the literature suggests X may help..."

CURRENT PHASE: [Chief Complaint | HPI | ROS | Differentiation | Recommendation]

OLDCARTS MNEMONIC: [generic reference, same every turn]

GATHERED SO FAR:
  Onset: [value or "(not yet asked)"]
  Location: [value or "(not yet asked)"]
  ... (all 9 dimensions)

CHIEF COMPLAINTS: [raw text + mapped symptoms/systems]

PERTINENT NEGATIVES: [denied systems]

CURRENT CANDIDATES: [top 5 with scores, quality, contraindications]

DIFFERENTIATING QUESTIONS: [from supplement KG diff walk]

SYSTEMS NOT YET REVIEWED: [list]

CONVERSATION SUMMARY: [compressed older turns]

RECENT TURNS: [last 4 exchanges]

MEDICATION CHECK REMINDER: [if candidates exist but meds not asked]

YOUR TASK THIS TURN: [phase-specific instructions]
```

### What Changes With the Intake KG

| Prompt Section | Currently | With Intake KG |
|----------------|-----------|----------------|
| OLDCARTS MNEMONIC | Generic, same for every symptom | Replaced by `SymptomProfile`-specific relevant dimensions |
| GATHERED SO FAR | Shows all 9 dimensions regardless | Only shows dimensions that `SymptomProfile` marks relevant |
| YOUR TASK THIS TURN | Hardcoded per-phase text | Generated from graph traversal — specific questions, with priority and rationale |
| DIFFERENTIATING QUESTIONS | From supplement KG diff walk only | Also includes intake KG-sourced discriminating questions per symptom |
| MEDICATION CHECK | Boolean flag + reminder text | `GraphAction:medication_check` with interaction knowledge from iDISK |
| Recommendation framing | LLM fabricates "why" | `GraphAction:fetch_mechanism` pulls real Mechanism of Action text from iDISK |

---

## Open Questions

*Items marked ✅ have been resolved through multi-model review.*

### Resolved

1. ✅ **Granularity of SymptomProfile**: **Hierarchical.** `ArchetypeProfile` (category) → `SymptomProfile` (specific). Profiles inherit from archetypes; top ~20 high-frequency symptoms get manual overrides. *(Gemini review)*

2. ✅ **Traversal engine**: **Expected Information Gain (EIG).** `effective_priority = base_priority × information_gain × system_relevance`. Not pure priority ordering. *(Gemini review)*

3. ✅ **How much is universal vs. symptom-specific?**: **~80% universal** (captured in ~10 ArchetypeProfiles), **~20% symptom-specific** (captured in manual overrides for top 20 symptoms). *(Gemini review)*

4. ✅ **iDISK CUI bridging**: **CUI mapping is mandatory.** Add CUIs to supplement KG nodes. Only reliable way to ensure cross-source entity resolution. *(Gemini review)*

5. ✅ **Storage**: **Same SurrealDB instance.** Use namespace/label separation (`intake:` vs `supp:`). Cross-graph queries via GraphAction are trivial. Separate DB adds latency for zero benefit. Intake KG stabilizes after v1; supplement KG grows independently. *(Grok review)*

6. ✅ **ResponsePattern matching**: **Reuse extractor output directly.** Map extraction fields → ClinicalGoal via `fulfills` edges. Don't duplicate classification. `ResponsePattern` as a first-class node is unnecessary indirection — extractor schema is the single source of truth. *(Grok review)*

7. ✅ **Implicit fulfillment**: **Post-extraction graph check.** After each extraction, run a lightweight sweep: "does current context satisfy any unfilled goals for active SymptomProfiles?" This is cheap and prevents the exact repetition bug. Extractor's job is to pull data; graph's job is to check what's satisfied. *(Grok review)*

8. ✅ **GraphAction as node vs. edge property**: **Keep as node.** Grok endorses the loose message-passing approach — GraphAction executor is the right seam for future extensibility (RAG layers, external APIs). Inspectability wins. *(Grok review, overriding Gemini suggestion)*

### Open

9. **Versioning**: Tag intake KG nodes with `min_supplement_kg_version` or use CUI-based references (more stable than string IDs). Grok suggestion — need to decide which approach.

10. **Adverse reaction reasoning — legal**: When the intake discovers a symptom matches a known adverse reaction of something the user is taking, how far can we go? "Stop taking X" is prescriptive. "Some of your symptoms are known side effects of X — discuss with your doctor" might be the safe framing. Need legal review.

11. **Product-level knowledge**: iDISK has 163,806 products with `has_ingredient` edges. If a user says "I take Nature Made Vitamin D," we could resolve that to specific DSI ingredients. Worth the complexity, or overkill for v1?

12. **iDISK Source_Description text**: The drug interaction edges have rich free-text descriptions of the interaction mechanism. Feed these directly into the recommendation prompt? Summarize with LLM first? Store as edge metadata?

13. **SymptomCluster population**: How many clusters do we need? LLM-assisted discovery + human review seems right. iDISK's shared adverse-reaction profiles may reveal some clusters automatically.

14. **Multi-symptom handling**: Graph must support parallel active SymptomProfiles with goal merging — one response can satisfy goals across multiple profiles. Without explicit handling, questions get duplicated or interactions get missed. *(Grok review)*

15. **Safety bypass prevention**: `required: true` + medication check must be non-bypassable at the engine level. A clever user or extraction error could skip safety gates. Need engine-level enforcement, not just edge metadata. *(Grok review)*

16. **`max_asks` edge metadata**: Cap repeated probing per goal even if not 100% fulfilled. Prevents the "ask alleviating factors 3 times" failure. How many is the right default? *(Grok review)*

17. **Debugging & testing infrastructure**: Graphs are opaque. Need (a) Graphviz/visualizer export, (b) synthetic patient simulation harness, (c) coverage metrics (% of ClinicalGoals reachable). Build early, not as afterthought. *(Grok review)*

18. **"YOUR TASK THIS TURN" removal**: With full graph traversal, the hardcoded task instructions in `context.rs` should disappear entirely. The graph traversal output *becomes* the task instruction. *(Grok review)*

---

## Multi-Model Review Log

### Gemini (2026-03-26) — Accepted Recommendations

| Recommendation | Status | Impact |
|---------------|--------|--------|
| Add `SymptomCluster` node type | ✅ Adopted | New node type for co-occurring symptom patterns |
| Expected Information Gain traversal | ✅ Adopted | `effective_priority = base_priority × information_gain × system_relevance` |
| CUI mapping is mandatory | ✅ Adopted | Resolved open question #8 |
| Archetype Profiles for population | ✅ Adopted | ~10 archetypes, LLM-assign 392 symptoms, manually tune top 20 |
| Fallback edges for silent stalls | ✅ Adopted | `falls_back` edge on every `QuestionTemplate` |
| `visited_nodes` registry | ✅ Adopted | Session-level `HashSet<NodeId>` prevents repeat traversals |
| `CausationInquiry` stage | ✅ Adopted | New `IntakeStage` for adverse reaction / de-prescribing checks |
| `GraphAction` as edge property | 🔄 Under evaluation | Open question #13 — fewer nodes but less inspectable |

**Notable Gemini insight**: "Most intake systems only look for solutions. By using iDISK's adverse reaction data, your Intake KG can perform a de-prescribing check." — This led to the `causation_inquiry` stage.

### Grok (2026-03-26) — Accepted Recommendations

| Recommendation | Status | Impact |
|---------------|--------|--------|
| Same SurrealDB instance | ✅ Adopted | Resolved storage question — namespace separation, no separate DB |
| Extractor schema as single source of truth for ResponsePattern | ✅ Adopted | `ResponsePattern` demoted from first-class node; extractor fields map directly to `fulfills` edges |
| Implicit fulfillment as post-extraction graph check | ✅ Adopted | Lightweight sweep after each extraction to auto-satisfy goals |
| Keep `GraphAction` as node (not edge property) | ✅ Adopted | Overrode Gemini's edge-property suggestion — inspectability and extensibility win |
| `max_asks` on edge metadata | ✅ Adopted | Caps repeated probing per goal |
| Multi-symptom parallel profiles | ✅ Adopted | Goal merging across active SymptomProfiles |
| Non-bypassable safety gates | ✅ Adopted | Engine-level enforcement of `required: true` safety edges |
| Synthetic patient test harness | ✅ Adopted | Build early for regression testing |
| Remove hardcoded "YOUR TASK THIS TURN" | ✅ Adopted | Graph traversal output replaces prompt-engineered task instructions |
| Behavior tree comparison | 📝 Noted | BT would be simpler for pure process, but graph wins on cross-symptom patterns + dynamic discriminators |

**Notable Grok insight**: "ResponsePattern as a first-class node adds indirection without much value — your extractor already produces structured fields. Make `fulfills` edges point directly from extractor schema fields to ClinicalGoal." — This simplifies the schema significantly.

**Notable Grok insight**: "A clever user or extraction error could skip medication check. Make `required: true` + SafetyGate non-ignorable at the engine level." — Safety must be structural, not advisory.

**Notable Grok divergence from Gemini**: Gemini suggested `GraphAction` as edge property; Grok says keep it as node. Grok's argument (inspectability, extensibility, future RAG layers) is stronger — adopted.

### GPT — (pending)
