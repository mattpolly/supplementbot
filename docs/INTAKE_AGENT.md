# Intake Agent — Final Design (Three-Model Consensus)

*Reviewed by Claude, Gemini, and Grok (2026-03-24). This is the synthesized design.*

## What Exists Today

### Knowledge Graph (graph-service)
- **66 nodes, 299 edges** from 8 supplements × 3 providers × 2 grade levels (5th, 10th)
- 14 node types, 14 edge types gated by continuous complexity lens (0.0–1.0)
- SurrealDB embedded (RocksDB), persistent at `~/.supplementbot/graph`
- Source tracking with quality tiers: Deduced < Speculative < SingleProvider < MultiProvider < CitationBacked

### Query Engine (graph-service/src/query.rs)
- Pattern-based traversal (not generic BFS):
  - `DirectSystem`: Symptom →[presents_in]→ System ←[acts_on]← Ingredient
  - `ViaMechanism`: Symptom →[presents_in]→ System ←[modulates]← Mechanism ←[via_mechanism]← Ingredient
- Path scoring: `geometric_mean(confidences) × quality_bonus(weakest_link) × length_bias(lens, len)`
- Three query types: `ingredients_for_symptom`, `ingredients_for_system`, `effects_of_ingredient`
- Proactive contraindication attachment on all recommendation results

### Legal Constraints (non-negotiable)
- **Never diagnose.** Never say "you have X condition." Never imply a medical condition.
- **Never say "cure."** Supplements address symptoms. They do not treat diseases.
- **No direct `relieves` edge.** Symptom → Ingredient is always indirect through System/Mechanism nodes.
- All output language framed as "supplements that act on systems where your symptoms present."

### Complexity Lens → Clinical Interview Mapping (from ROADMAP.md)
- **Chief Complaint** = 5th-grade level (0.15) — "I can't sleep"
- **HPI** = relational/intermediate (~0.3–0.5) — "started when I began working nights"
- **ROS** = system-by-system sweep at lens level matching detail gathered

---

## The Problem

The query engine answers structured questions: "what ingredients address muscle cramps?" But a real user says "my legs hurt at night and I can't sleep." The intake agent must:

1. Translate natural language into graph concepts (symptoms, systems)
2. Gather enough clinical context to differentiate between candidate ingredients
3. Present results with reasoning chains, not just ingredient names
4. Never cross the legal line into diagnosis

This is **not** a chatbot with a lookup step at the end. The graph must steer the conversation in real time.

---

## Final Design

### Architecture Overview

```
┌──────────────────────────────────────────────────────┐
│                  Web Frontend (chat UI)               │
│              supplementbot.com (HTML/JS)              │
└─────────────────────┬────────────────────────────────┘
                      │ WebSocket / SSE
┌─────────────────────▼────────────────────────────────┐
│                  Web Server (Rust, axum)              │
│                 supplementbot.com/                    │
│                                                      │
│  ┌─────────────────────────────────────────────────┐ │
│  │              Intake Agent (new crate)            │ │
│  │                                                 │ │
│  │  ┌───────────┐  ┌────────────┐  ┌───────────┐  │ │
│  │  │  Session   │  │  Context   │  │ Candidate │  │ │
│  │  │  State     │  │  Generator │  │ Tracker   │  │ │
│  │  │ (OLDCARTS) │  │  (prompts) │  │ (graph)   │  │ │
│  │  └───────────┘  └────────────┘  └───────────┘  │ │
│  │         │              │              │         │ │
│  │         ▼              ▼              ▼         │ │
│  │  ┌─────────────────────────────────────────┐   │ │
│  │  │           LLM (conversational)          │   │ │
│  │  └─────────────────────────────────────────┘   │ │
│  └─────────────────────────────────────────────────┘ │
│                          │                           │
│  ┌───────────────────────▼───────────────────────┐   │
│  │  graph-service (KnowledgeGraph + QueryEngine)  │  │
│  └────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

### New Crate: `intake-agent`

The clinical reasoning engine. Does NOT generate text — decides *what to do next* and builds structured context for the LLM to render into natural language.

#### Session State

Tracks a single intake conversation. Persisted per-session (not in the graph).

```rust
pub struct IntakeSession {
    pub id: Uuid,
    pub phase: IntakePhase,
    pub chief_complaints: Vec<ChiefComplaint>,
    pub oldcarts: OldcartsState,
    pub systems_reviewed: HashSet<String>,
    pub systems_denied: HashSet<String>,    // pertinent negatives (Gemini)
    pub candidates: CandidateSet,
    pub lens_level: f64,                    // escalates as detail accumulates
    pub turns: Vec<Turn>,                   // conversation history
    pub turn_summary: Option<String>,       // compressed history after ~8 turns (Grok)
    pub contraindications: Vec<String>,     // disclosed conditions/meds
}

pub enum IntakePhase {
    ChiefComplaint,     // "What brings you in today?"
    Hpi,                // OLDCARTS deep-dive on each CC
    ReviewOfSystems,    // Graph-guided system sweep
    Differentiation,    // Narrowing candidates via discriminating questions
    Recommendation,     // Final results presentation
}

// Phase transitions are not strictly linear — Differentiation loops
// back on itself as long as high-value differentiators remain AND the
// user is engaged. Transition to Recommendation when:
// 1. Differentiators are exhausted (graph has nothing useful left to ask)
// 2. User signals disengagement (short answers, "I don't know", explicit request)
// 3. User explicitly asks for recommendations
// Every question-answer pair is valuable training data for the symptom
// ontology, so we encourage depth — but only with good questions.

pub struct OldcartsState {
    pub onset: Option<String>,
    pub location: Option<String>,
    pub duration: Option<String>,
    pub character: Option<String>,
    pub aggravating: Vec<String>,
    pub alleviating: Vec<String>,
    pub radiation: Option<String>,
    pub timing: Option<String>,
    pub severity: Option<u8>,       // 1-10
}

pub struct ChiefComplaint {
    pub raw_text: String,
    pub mapped_symptoms: Vec<String>,       // graph node names
    pub mapped_systems: Vec<String>,        // graph node names
    pub associated_symptoms: Vec<String>,   // accompanying symptoms from HPI (Gemini)
}
```

#### Red Flag Ejector (unanimous)

Hard-coded safety check that runs BEFORE any graph reasoning or LLM call. Pattern-matches on emergency keywords in user input:

```rust
const RED_FLAGS: &[&str] = &[
    "chest pain", "heart attack", "stroke", "can't breathe",
    "suicidal", "want to die", "kill myself", "overdose",
    "sudden numbness", "vision loss", "severe bleeding",
    "allergic reaction", "anaphylaxis", "seizure",
];

pub enum SafetyCheck {
    Clear,                    // proceed with normal intake
    EmergencyExit(String),    // stop everything, show emergency resources
}
```

When triggered: immediately break intake flow, display high-contrast emergency resource block (911, crisis hotline, "seek immediate medical attention"). This is a static, pre-written UI component — NOT LLM-generated. Session is flagged; no further supplement discussion occurs.

#### Candidate Tracker

Maintains a ranked set of ingredient candidates with graph evidence and differentiators.

```rust
pub struct CandidateSet {
    pub candidates: Vec<Candidate>,
}

pub struct Candidate {
    pub ingredient: String,
    pub per_symptom_scores: HashMap<String, f64>,  // symptom → score from QueryEngine
    pub composite_score: f64,                       // intersection + coverage bonus
    pub supporting_paths: Vec<TraversalPath>,       // why this candidate
    pub differentiators: Vec<Differentiator>,       // what would strengthen/weaken it
    pub quality: EdgeQuality,
}

/// A question that would help distinguish between candidates
pub struct Differentiator {
    pub question_topic: String,       // e.g., "neurological symptoms"
    pub favors: Vec<String>,          // ingredients this would support
    pub disfavors: Vec<String>,       // ingredients this would weaken
    pub graph_basis: String,          // e.g., "magnesium acts_on nervous system; calcium does not"
    pub entropy_score: f64,           // how evenly this splits the candidate set (Grok)
}
```

**Re-run QueryEngine each turn** (Claude + Grok consensus). The graph is small and traversal is fast. When new symptoms are mapped, re-run queries for the full symptom set to ensure candidates reflect the complete picture. On top of the base QueryEngine scores, the intake agent layers:
- **Intersection gate** — candidate must appear in at least one query per chief complaint
- **Coverage bonus** — `score × (1 + coverage_fraction × 0.3)` (Grok)
- **Negative evidence penalty** — for denied systems (pertinent negatives)
- **Contraindication elimination** — for disclosed conditions

#### Multi-Symptom Scoring (Grok — accepted)

```rust
pub fn score_candidates(
    per_symptom_results: &[Vec<RecommendationResult>],
) -> Vec<Candidate> {
    // 1. Intersection: only keep ingredients that appear in ALL symptom result sets
    // 2. For each surviving ingredient, sum per-symptom best scores
    // 3. Apply coverage bonus: score × (1 + 0.3 × coverage_fraction)
    //    where coverage_fraction = symptoms_covered / total_symptoms
    // 4. Sort descending
}
```

Why intersection + coverage over geometric mean: geometric mean is too aggressive on secondary symptoms. An ingredient that's exceptional for the primary CC but merely decent for a secondary symptom shouldn't be penalized as harshly as geometric mean would. Intersection ensures the ingredient is *relevant* to all symptoms; the coverage bonus rewards breadth without punishing depth.

#### Differentiator Computation

**Depth-aware differentiation** (Gemini + Grok consensus):

Given candidates {Magnesium, Calcium} both acting on the muscular system:
1. For each candidate, collect all systems it acts_on
2. Find systems that are NOT shared — these are discriminating
3. If candidates share ALL systems, walk one hop deeper: compare Mechanisms
4. If they share Mechanisms, compare Pathways/Substrates
5. The search walks down until it finds a divergence point

This naturally drives lens escalation — deeper differentiating questions pull the conversation to higher complexity levels.

**Entropy-reduction sort** (Grok, nice-to-have for v1):
```
differentiator.entropy_score = (favors.len() / total_candidates) × (1 - shared_fraction)
```
Prefer questions that split the candidate list closest to 50/50. Not required for v1 but improves question quality when the candidate set is large.

#### Pertinent Negatives (Gemini)

When the user explicitly denies a system ("no, I don't have any digestive issues"), recorded in `systems_denied`. Candidates depending on that system get a **negative evidence penalty**:

```rust
pub struct NegativeEvidence {
    pub system: String,
    pub affected_candidates: Vec<String>,
    pub penalty_factor: f64,       // e.g., 0.7 — reduces candidate score
}
```

A denied system doesn't eliminate a candidate (the supplement may still help through other paths), but weakens evidence proportionally to how much of its score came from the denied system.

#### User Correction Handling (Grok)

If the user says "actually it's not cramps, it's tingling," the agent must:
1. Remove the old mapped symptom from the active CC
2. Map the corrected text to new graph nodes
3. Re-run the full candidate scoring pipeline
4. Update differentiators for the new candidate set

```rust
impl IntakeSession {
    pub fn revise_complaint(&mut self, old_text: &str, new_text: &str);
}
```

#### Context Generator

Rebuilds the LLM system prompt each turn. This is the "regrounding" mechanism.

```rust
pub struct IntakeContext {
    pub system_prompt: String,   // rebuilt every turn
    pub user_message: String,    // the user's latest input
}

impl IntakeContext {
    /// Build a fresh system prompt from current session state.
    /// Returns a plain String — model-agnostic, no provider-specific tooling.
    pub fn build(session: &IntakeSession, graph_context: &GraphContext) -> Self;
}
```

**System prompt structure (rebuilt every turn):**

```
ROLE:
You are a supplement intake specialist. You gather information about
a person's symptoms to identify supplements that may help.

LEGAL CONSTRAINTS:
- Never diagnose. Never say "you have X."
- Never say "cure." Supplements address symptoms, not diseases.
- Frame everything as "supplements that act on the systems where
  your symptoms present."
- If the user describes an emergency, direct them to a doctor.

CURRENT PHASE: {phase}

MNEMONIC — OLDCARTS (Review of Symptoms):
  O: Onset — When did this start?
  L: Location — Where exactly?
  D: Duration — How long does it last?
  C: Character — What does it feel like?
  A: Aggravating/Alleviating — What makes it better/worse?
  R: Radiation — Does it spread?
  T: Timing — Pattern? Time of day?
  S: Severity — 1-10?

GATHERED SO FAR:
{oldcarts_state — filled fields and gaps}

CHIEF COMPLAINTS:
{mapped symptoms and systems, including associated symptoms}

PERTINENT NEGATIVES:
{systems explicitly denied by user}

CURRENT CANDIDATES (ranked):
{top N candidates with scores, supporting paths, and quality tiers}

DIFFERENTIATING QUESTIONS AVAILABLE:
{computed differentiators — sorted by entropy score}

SYSTEMS NOT YET REVIEWED:
{systems adjacent to candidates that haven't been asked about}

CONVERSATION SUMMARY:
{compressed history of earlier turns, if > 8 turns}

YOUR TASK THIS TURN:
{phase-specific instruction — e.g., "Ask about the next OLDCARTS
 dimension" or "Ask the top differentiating question" or
 "Ask if the user is ready for recommendations" or
 "Present the final recommendations with reasoning chains"}
```

The key insight: **the LLM is a language renderer, not the reasoner** (unanimous). The graph topology + session state determine what to ask. The LLM turns that into natural conversation. Every recommendation traces to concrete graph paths — fully auditable.

#### Post-Generation Safety Filter (unanimous)

Prompt-level legal constraints are necessary but NOT sufficient. A deterministic post-generation filter scans every LLM response before it reaches the user:

```rust
pub struct SafetyFilter {
    pub blacklist_patterns: Vec<Regex>,  // "you have", "diagnose", "cure", "treats [condition]"
    pub replacement: String,
}

pub enum FilterResult {
    Pass(String),           // safe to send
    Rewrite(String),        // violation found, re-prompt with stricter instruction
    Block,                  // severe violation, fall back to canned response
}
```

The disclaimer ("This is not medical advice. Please consult a healthcare provider.") is injected at the **UI level** — physically outside the LLM's output — so it cannot be "forgotten" or omitted by the model. (Gemini + Grok)

### Phase Flow

```
User: "My legs hurt at night and I can't sleep"
                    │
              ┌─────▼─────┐
              │ Red Flag   │──→ Emergency? → Static emergency UI
              │ Check      │
              └─────┬──────┘
                    │ Clear
                    ▼
            ┌──────────────┐
            │ Chief        │  Map "legs hurt" → Symptom nodes
            │ Complaint    │  Map "can't sleep" → Symptom nodes
            │              │  Query graph for initial candidates
            └──────┬───────┘
                   │
                   ▼
            ┌──────────────┐
            │ HPI          │  OLDCARTS for each CC
            │ (OLDCARTS)   │  Each answer may refine candidates
            │              │  Capture associated symptoms
            │              │  "Worse at night" → timing pattern
            └──────┬───────┘
                   │
                   ▼
            ┌──────────────┐
            │ Review of    │  Graph-guided: systems adjacent to
            │ Systems      │  candidates, ask about uncovered ones.
            │              │  Record pertinent negatives.
            │              │  "Any digestive issues?" → denied →
            │              │  penalize GI-dependent candidates
            └──────┬───────┘
                   │
                   ▼
            ┌──────────────┐
            │ Differential │  Depth-aware differentiators.
            │              │  "Any tingling or numbness?" separates
            │              │  Magnesium from Calcium.
            │              │
            │              │  Loop while:
            │              │   - good differentiators remain AND
            │              │   - user is giving substantive answers
            │              │
            │              │  Exit when:
            │              │   - differentiators exhausted, OR
            │              │   - user disengaged (short answers,
            │              │     "I don't know"), OR
            │              │   - user asks for recommendations
            └──────┬───────┘
                   │
                   ▼
            ┌──────────────┐
            │ Recommend-   │  Present top candidates with:
            │ ation        │  - Reasoning chain (from traversal paths)
            │              │  - Quality tier
            │              │  - Contraindications
            │              │  - "Talk to your doctor" (UI-level)
            └──────────────┘
```

**Lens escalation during intake:** The lens starts at 0.15 (5th grade) during CC. As OLDCARTS fills in, the lens escalates — if the user describes mechanism-level detail ("it feels like a nerve thing"), the lens can jump to 0.5+ to access Substrate/Pathway nodes. The lens level is a function of how much detail has been gathered, not a fixed schedule.

**Context window management** (Grok): After ~8 turns, compress older turns into a single "Conversation Summary" string included in the system prompt. Keeps token usage bounded for self-hosted models with smaller context windows.

### Concept Mapping: Free Text → Graph Nodes (unanimous: hybrid)

Three-tier approach:

1. **Exact/alias match** (no LLM, ~40% of cases) — String match + merge table lookup. If "leg cramps" is in the graph or aliased to "muscle cramps", direct hit.

2. **Embedding similarity** (no LLM) — SurrealDB supports vector indexes natively. Embed user text, query against Symptom/System node embeddings. Threshold ~0.78 cosine (Grok).

3. **LLM ranker** (fallback, only when step 2 returns multiple low-confidence matches) — Feed the LLM the top 5 closest graph nodes: "Which of these best describes the user's intent?" Constrained to existing ontology — cannot invent new nodes. (Gemini)

### Web Layer

**Backend (`supplementbot.com/`):**
- Rust + axum (lightweight, async, same ecosystem)
- WebSocket endpoint for chat (`/ws/chat`)
- REST endpoint for session management (`/api/session`)
- Serves static frontend files
- Loads KnowledgeGraph + SourceStore + MergeStore at startup (shared state)

**Frontend:**
- Minimal HTML/CSS/JS — no framework for v1
- Chat bubble UI
- Typing indicator during LLM calls
- Expandable reasoning chains ("why this supplement?" → shows graph path) (Grok: "Why this question?" cards too)
- Disclaimer banner always visible (UI-level, not LLM-generated)

**Session management:**
- In-memory HashMap for v1 (sessions keyed by UUID)
- 30-minute inactivity timeout → auto-archive session (Grok)
- Move to Redis/DB when we need persistence across restarts

---

## Decisions (Three-Model Consensus)

| Question | Decision | Who Agreed |
|----------|----------|------------|
| Clinical workflow | OLDCARTS → ROS → Differentiation (loop while productive) → Recommendation | All three |
| Pertinent negatives | `systems_denied` with score penalty | Gemini (proposed), all agreed |
| Associated symptoms | Tracked per CC, can shift system mapping | Gemini (proposed), all agreed |
| Differentiator depth | Walk deeper (System → Mechanism → Pathway) until divergence found | Gemini (proposed), Grok enhanced with entropy sort |
| Safety layers | Three-layer: red flag ejector → prompt constraints → post-gen regex filter | All three |
| UI disclaimer | Injected at UI level, not LLM-generated | Gemini + Grok |
| Concept mapping | Hybrid: exact/alias → embedding → LLM ranker | All three |
| LLM role | Pure renderer — no graph access, full traceability | All three |
| Candidate scoring | Re-run QueryEngine each turn + intake adjustments | Claude + Grok |
| Multi-symptom scoring | Intersection gate + sum of scores + coverage bonus (Grok) | Grok (accepted by user) |
| Stopping criteria | Mandatory OLDCARTS + keep going while questions are good and user is engaged | Revised post-review |
| Ontology gaps | Runtime discovery from patient conversations; batch extraction later | Grok (accepted by user) |
| User corrections | `revise_complaint()` — remap + re-score | Grok (accepted) |
| Session timeout | 30 min auto-archive | Grok (accepted) |
| Context management | Summarize turns after ~8 into compressed summary | Grok (accepted) |
| Self-hosted model | Context generator returns plain String, model-agnostic | Grok |
| Export recommendations | JSON + Markdown for doctor visits (nice-to-have) | Grok |

---

## What We're NOT Building Yet

- **User accounts / authentication** — single-user prototype first
- **Persistent session history** — in-memory for v1
- **Dosage recommendations** — out of scope, additional legal complexity
- **Interaction checking against user's current medications** — future, requires medication ontology
- **Mobile app** — web-first
- **Proactive symptom ontology** — will be built reactively from real patient conversations

---

## Known Gaps / Future Work

1. **Symptom→System coverage** — The graph is ingredient-outward. The intake agent will discover gaps at runtime when users mention symptoms that don't map to any graph node. These unmapped symptoms should be logged with context for later batch extraction.

2. **Entropy-optimized differentiators** — v1 uses simple non-shared-edge differentiators sorted by candidate split ratio. Future: full information-theoretic question selection maximizing entropy reduction per turn.

3. **Export recommendations** — JSON + Markdown export for users to share with healthcare providers. Nice-to-have for v1.

4. **"Why this question?" transparency** — Expandable cards showing the graph basis for each question the agent asks. Builds user trust.

5. **Dual-graph alignment (Reflection Modeling)** — Long-term architectural evolution: the patient's session state (symptoms, OLDCARTS, denied systems, engagement signals) becomes a first-class temporal knowledge graph. At query time, a hybrid alignment engine computes embedding similarity + graph traversal overlaps between the patient graph and the supplement KG, producing "reflection bridges" that quantify alignment. Current candidate scoring is a primitive form of this; future iterations should move toward graph-matching rather than path aggregation.
