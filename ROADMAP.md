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

## Clinical Intake Maps to Complexity Lens

The standard medical interview structure maps naturally to the lens:
- **Chief Complaint** = 5th-grade level ("I can't sleep")
- **HPI** = relational/intermediate ("started when I began working nights, caffeine makes it worse")
- **ROS** = system-by-system sweep filling in what the patient didn't volunteer

Could potentially run the lens in reverse — start at low complexity to match CC to broad system/property nodes, then escalate as the conversation gathers detail.

## General Architecture Validation

The following design decisions are already protecting future evolution:
- Continuous complexity dial (not discrete enum) allows precise tuning and new types without modifying variants
- Epoch system enables re-evaluation when the lens changes
- Open `extra: HashMap<String, String>` on edge metadata accommodates future dimensions
- Dual enforcement (prompt guidance + parser rejection) prevents advanced concepts from leaking into simple explanations
- Affordance-based reasoning ("affords sleep quality" not "cures insomnia") keeps semantics rich while avoiding medical claims

**Strategy: ship the single-ingredient pipeline, prove it works, then widen the lens.**