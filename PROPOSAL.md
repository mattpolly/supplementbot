# Supplementbot — System Proposal

## What This Is

A neurosymbolic AI (NSAI) tool for systemic wellness. It maps how nutraceuticals interact with
the human body by combining LLM-based extraction with a structured knowledge graph (the symbolic
engine). The goal is a simplified Clinical Decision Support System (CDSS) that takes a user's
symptoms and finds broad-spectrum supplements that address multiple physiological systems
simultaneously.

**Legal constraint (non-negotiable):** We are not doctors. This system never diagnoses. It never
uses the word "cure." All language is framed around symptoms and treatments — not medicines and
diseases.

---

## Core Concept: Affordance-Based Reasoning

Instead of rigid "symptom → supplement" lookups, we model what supplements *enable the body to
do*. For example: "Magnesium affords muscle relaxation." These affordances are traced across
interconnected physiological systems to find broad-spectrum matches.

---

## Architecture: Three Layers

### Layer 1 — Perceptual (Neural)
Maps fuzzy natural language into discrete symbols the symbolic engine can reason about.

- **Input**: User symptom descriptions, or LLM responses about nutraceuticals
- **Function**: Entity extraction and structured parsing — symptoms, ingredients, mechanisms
- **Tech**: Anthropic + OpenAI APIs via `reqwest`/`tokio` (no local models needed)

### Layer 2 — Context (Symbolic Engine / Knowledge Graph)
Stores what the system knows and how things relate. This is the brain that grows over time.

- **Structure**: Typed nodes (Ingredient, System, Mechanism, Symptom, Property) connected by
  typed edges (acts_on, via_mechanism, affords, presents_in, contraindicated_with, modulates)
- **Edge metadata**: confidence, source (extracted vs. structurally emergent), iteration count,
  llm_agreement
- **Tech**: `petgraph` (in-memory, Rust-native), serialized to JSON/GraphML

### Layer 3 — Logic (Symbolic Reasoning)
The auditor. Enforces constraints, checks safety, makes final decisions.

- **Function**: Contraindication checking, safety constraints, recommendation ranking
- **Tech**: `egg` (Rust-native e-graph library) or Z3 Rust bindings

---

## The Bootstrap Loop — How the Symbolic Engine Learns

The knowledge graph starts empty. It is populated through an iterative loop where the LLM teaches
the symbolic engine, and the symbolic engine generates new hypotheses for the LLM to evaluate.
This is the core of the project.

```
┌─────────────────────────────────────────────────────┐
│                  BOOTSTRAP LOOP                      │
│                                                      │
│  ┌──────────┐    extract     ┌──────────────────┐   │
│  │   LLM    │ ─────────────► │  Symbolic Engine │   │
│  │(Teacher) │                │  (Knowledge Graph│   │
│  │          │ ◄───────────── │   grows here)    │   │
│  └──────────┘    evaluate    └──────────────────┘   │
│        ▲          claims           │                 │
│        │                          │ topology         │
│        │                          │ analysis         │
│        │                          ▼                  │
│        └──────────────── speculative inference       │
└─────────────────────────────────────────────────────┘
```

### Phase 1: Curriculum Extraction (LLM → Graph)
An agent feeds nutraceuticals through a curriculum of questions, simple to complex.

**Stage 1 — Foundational:**
- "What physiological systems does [magnesium] act on?"
- "What are the known mechanisms of action?"
- "What are the primary therapeutic uses?"
- Produces: basic nodes and edges

**Stage 2 — Relational:**
- "How does [magnesium]'s effect on smooth muscle relate to its neurological properties?"
- "What are the contraindications when combined with [calcium channel blockers]?"
- Produces: cross-system links, safety constraints, interaction data

### Phase 2: Speculative Inference (Graph → Candidates)
The symbolic engine examines its own topology and generates candidate claims that the LLM never
explicitly stated — patterns that emerge from graph structure alone.

- "Compounds A and B both affect pathway X through different mechanisms — their combination
  may afford Y."
- Tagged as `source: structurally_emergent`

### Phase 3: LLM Review (Candidates → Confidence)
Speculative claims go back to the LLMs (both Claude and GPT for stronger signal), phrased
multiple ways. Each claim is tagged:

- **Confirmed**: LLMs agree and can cite mechanisms → added to graph
- **Plausible**: LLMs say reasonable but can't confirm → *discovery track* (most valuable)
- **Contested/Rejected**: LLMs disagree → downgraded or removed

Structurally emergent claims that land in "plausible" are the system's most valuable output.
They represent novel inferences — things the graph found that the LLM didn't explicitly teach.

### Phase 4: Iterate
Each cycle adds validated edges, refines speculative claims, and generates more sophisticated
hypotheses as the graph grows denser. Coverage matters more than quality on any single pass.
The loop is the filter.

---

## Observability

Every data exchange in the bootstrap loop is logged:
- Prompt sent to LLM (with nutraceutical, stage, question)
- Raw LLM response
- Parsed graph operations (nodes/edges added or updated)
- Speculative claims generated (with topology justification)
- LLM review results (confidence tag, which LLMs agreed)

This lets you watch the symbolic engine learn in real time.

---

## Chatbot: Intake Interface

A conversational intake agent that conducts a structured symptom interview before handing off to
the recommendation engine. The chatbot is graph-informed in real time — as symptoms surface, the
graph is already traversing quietly in the background to guide what to ask next.

### Session State (maintained throughout conversation)
```
ChiefComplaint:     String          // Anchor — never forgotten
HPS:                Vec<Symptom>    // History of Present Symptoms
SystemsImplicated:  Vec<System>     // Updated live as symptoms surface
Medications:        Vec<String>     // Current medications (for contraindication check)
Supplements:        Vec<String>     // Current supplements (for redundancy/interaction check)
CollectionFlags: {
    hps_complete:   bool,
    meds_collected: bool,
    supps_collected:bool,
    ros_complete:   bool,
}
```

### Interview Flow
1. **Chief Complaint** — one sentence, what brought them here
2. **History of Present Symptoms (HPS)** — onset, duration, character, what helps/worsens
3. **Review of Systems (ROS)** — guided by CC and HPS; the graph informs which systems to probe
4. **Current medications** — feeds contraindication checking
5. **Current supplements** — avoids redundancy, checks interactions

### Re-Grounding
The chatbot checks session state every turn. If the conversation drifts from the CC, it steers
back. Once all `CollectionFlags` are true, it stops asking and hands off to the recommendation
engine. The chatbot is warmly directive — it has a job and keeps moving toward it.

### Sensitivity and Specificity
- **Sensitive**: ROS casts wide enough to catch unexpected cross-system signals
- **Specific**: Questions are justified by what's already been revealed — no noise
- Unexpected symptoms are surfaced and traced; an offhand mention can unlock a better
  recommendation

---

## Query Workflow (Recommendation Engine)

Once intake is complete:

1. **Normalization**: Symptoms mapped to canonical graph nodes (SNOMED-CT vocabulary for
   symptom terms at intake boundary only)
2. **Discovery**: Symbolic engine traverses graph to find broad-spectrum ingredients with
   affordances spanning the implicated systems
3. **Safety check**: Contraindication edges checked against medications and current supplements
4. **Ranking**: Ingredients scored by affordance breadth across implicated systems
5. **Output**: Ranked suggestions with full affordance traces — *why* each supplement was
   suggested and which symptoms/systems it addresses

---

## Starting Scope

### Physiological Systems (4)
1. **Nervous** — neurotransmitter modulation, NMDA, GABA, HPA axis
2. **Gastrointestinal** — motility, gut barrier, microbiome, smooth muscle
3. **Musculoskeletal** — muscle contraction/relaxation, inflammation, mineral balance
4. **Immune** — innate/adaptive signaling, cytokine modulation, oxidative stress

### Starting Nutraceuticals (10)
Magnesium, Zinc, Vitamin D, Omega-3 fatty acids, B-complex vitamins, Vitamin C, Curcumin,
Probiotics, Ashwagandha, CoQ10

---

## Tech Stack

| Component | Technology |
|---|---|
| Language | Rust |
| Async runtime | `tokio` |
| HTTP / LLM API calls | `reqwest` |
| Serialization | `serde` / `serde_json` |
| Knowledge graph | `petgraph` |
| Graph serialization | JSON / GraphML |
| Symbolic reasoning / safety | `egg` (e-graph, Rust-native) |
| LLM providers | Anthropic (Claude) + OpenAI (GPT) |
| Symptom vocabulary | SNOMED-CT (intake normalization only) |

---

## Project Structure

```
supplementbot/
├── Cargo.toml
└── src/
    ├── main.rs
    ├── llm/            # API clients — Anthropic + OpenAI
    ├── graph/          # Schema types + petgraph wrapper + serialization
    ├── curriculum/     # Question pipeline (Stage 1 foundational → Stage 2 relational)
    ├── extraction/     # LLM response → typed graph nodes/edges
    ├── inference/      # Speculative inference from graph topology
    ├── review/         # Multi-LLM confidence rating pipeline
    ├── loop_runner/    # Orchestrates the full bootstrap cycle
    ├── chatbot/        # Intake agent + session state
    └── query/          # Graph traversal → safety check → ranked output
```

---

## Build Order

### Phase 1 — Foundation: Graph Schema
Define all Rust types: node enums, edge enums, metadata structs (confidence, source, iteration,
llm_agreement). Wrap `petgraph`. Add JSON serialization. Nothing runs yet — just the data model,
locked in and type-safe.

*This is the symbolic engine's skeleton.*

### Phase 2 — LLM Clients + Curriculum Agent
Async clients for Anthropic and OpenAI. Curriculum agent generates Stage 1 questions, calls the
LLM, returns raw responses. Test case: Magnesium, Stage 1 only.

*First data exchange — you'll be able to watch the LLM respond.*

### Phase 3 — Extraction Parser
Converts raw LLM responses into typed graph nodes and edges. The bridge between the neural and
symbolic layers. Stage 1 only first (direct claims). Stage 2 (relational) follows once Stage 1
is solid.

*First time the symbolic engine learns something.*

### Phase 4 — Speculative Inference Engine
Analyzes graph topology to generate candidate claims the LLM never explicitly made. Tags
everything as `structurally_emergent`.

*First time the symbolic engine reasons on its own.*

### Phase 5 — Review Pipeline + Loop Orchestration
Sends speculative claims to both LLMs with varied phrasings. Parses confidence ratings. Writes
results back to graph with metadata. Wires the full loop with coverage heuristics.

*The NSAI loop is alive.*

### Phase 6 — Chatbot Intake
Conversational intake agent with session state. Re-grounding logic. Live graph traversal during
interview to guide ROS questions. Hands off to recommendation engine when complete.

### Phase 7 — Recommendation Engine
Symptom normalization → graph traversal → safety check → ranked output with affordance traces.

---

## What This Demonstrates (Portfolio)

- Neurosymbolic AI architecture — combining neural (LLM) and symbolic (graph) reasoning
- Agentic loop design — LLM as teacher/oracle, graph as the reasoning system that grows
- Knowledge graph construction and traversal in Rust
- Multi-LLM orchestration with consensus scoring
- Novel inference discovery — structurally emergent claims as a discovery mechanism
- Clinical-grade intake design with legal compliance built in
