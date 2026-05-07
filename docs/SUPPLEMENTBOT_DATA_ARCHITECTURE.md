# SupplementBot Data Architecture — May 2026

## Purpose

This document is a companion to [DATA_ARCHITECTURE_REVIEW.md](DATA_ARCHITECTURE_REVIEW.md).
It describes supplementbot's SurrealDB graph in detail — schemas, row counts, quality
characteristics, and where the data maps cleanly vs. where there are friction points
going into supplementology's Postgres.

---

## SurrealDB Overview

- **Instance**: native process, RocksDB at `/srv/www/supplementbot/data/graph-server`
- **URL**: `ws://localhost:8000`
- **Namespace**: `supplementbot`, **Database**: `supplementbot`
- **Version**: SurrealDB 3.0.5

Shared instance: the `supplementconsult` namespace lives on the same process.

---

## Table Inventory

| Table | Rows | Description |
|---|---|---|
| `node` | 213 | NSAI concept graph nodes |
| `edge` | 1,794 | NSAI graph edges between nodes |
| `edge_source` | 15,369 | LLM provenance observations per edge |
| `node_source` | 213 | LLM provenance records per node (one per node) |
| `edge_citation` | 60,306 | Citation-backed evidence for edges (SuppKG + supplementology confirm-edges) |
| `ingredient_registry` | 19 | Small registry for NSAI loop ingredient lookups |

No `node_alias` table exists. The `ingredient_registry` is a lightweight lookup cache, not
a synonym database.

---

## Table Schemas and Samples

### `node`

```
Fields: id, name, node_type
```

**`node_type`** is a tagged enum stored as a nested object:
```json
{ "Ingredient": {} }
{ "Mechanism": {} }
{ "Property": {} }
{ "Symptom": {} }
{ "System": {} }
```

**Node type counts:**
| Type | Count |
|---|---|
| Ingredient | 97 |
| Property | 38 |
| Mechanism | 37 |
| Symptom | 27 |
| System | 14 |
| **Total** | **213** |

**Sample:**
```json
{ "id": "node:magnesium", "name": "magnesium", "node_type": { "Ingredient": {} } }
{ "id": "node:autophagy", "name": "autophagy", "node_type": { "Mechanism": {} } }
{ "id": "node:anti-inflammatory_effect", "name": "anti-inflammatory effect", "node_type": { "Property": {} } }
{ "id": "node:cardiovascular_system", "name": "cardiovascular system", "node_type": { "System": {} } }
{ "id": "node:achy", "name": "achy", "node_type": { "Symptom": {} } }
```

**Full vocabulary:**

*Ingredient nodes (97):* These map to canonical supplementology entities.
acetyl-l-carnitine, alpha-lipoic acid, ashwagandha, berberine, boron, bromelain,
calcium, chromium, coenzyme q10, collagen, copper, creatine, curcumin, dhea, epa/dha,
garlic, ginger, ginkgo biloba, ginseng, glucosamine, glutamine, green tea extract,
hyaluronic acid, iodine, iron, l-arginine, l-carnitine, l-citrulline, l-glutamine,
l-lysine, l-theanine, lion's mane, lutein, lycopene, magnesium, melatonin, methyl b12,
milk thistle, myo-inositol, n-acetyl cysteine, nattokinase, niacin, nmn, omega-3 fatty acids,
pantothenic acid, phosphatidylserine, potassium, probiotics, pygeum, quercetin, resveratrol,
riboflavin, s-adenosyl methionine, selenium, silymarin, spirulina, tart cherry, thiamine,
turmeric, ubiquinol, valerian root, vitamin a, vitamin b1, vitamin b12, vitamin b2,
vitamin b3, vitamin b5, vitamin b6, vitamin b9, vitamin c, vitamin d, vitamin d3, vitamin e,
vitamin k, vitamin k2, zinc... (97 total)

*Mechanism nodes (37):* autophagy, antioxidant production, bile production, blood clotting,
collagen synthesis, cortisol regulation, dna repair, dopamine production, energy metabolism,
fat metabolism, glucose metabolism, hormone regulation, immune modulation, inflammation response,
insulin sensitivity, lipid metabolism, mitochondrial function, neurotransmitter synthesis,
nitric oxide production, protein synthesis, serotonin production, sleep cycle regulation,
testosterone production... (37 total)

*Property nodes (38):* adaptogenic effect, anti-aging effect, anti-inflammatory effect,
antioxidant activity, antimicrobial effect, anxiolytic effect, bone strength, cardiovascular
support, cognitive function, detoxification support, digestive health, energy levels, endurance,
fat loss, focus, heart health, immune support, joint health, libido, liver health, mental clarity,
mood stabilization, muscle recovery, muscle strength, neuroprotection, pain relief, relaxation,
sleep quality, stress reduction, testosterone levels, thyroid support, weight management... (38 total)

*System nodes (14):* cardiovascular system, central nervous system, digestive system,
endocrine system, immune system, integumentary system, lymphatic system, musculoskeletal system,
reproductive system, respiratory system, skeletal system, urinary system... (14 total)

*Symptom nodes (27):* achy, anxiety, brain fog, constipation, depression, fatigue, headaches,
high blood pressure, high cholesterol, inflammation, insomnia, irregular heartbeat, joint pain,
low energy, low mood, memory loss, muscle cramps, muscle weakness, nausea, nerve pain,
oxidative stress, poor digestion, poor focus, reduced urinary frequency, stress, swelling,
weakness... (27 total)

---

### `edge`

```
Fields: id, in (node ref), out (node ref), edge_type (tagged enum), metadata
```

**`metadata` fields:**
- `source`: tagged enum — `{ "Extracted": {} }`, `{ "Deduced": {} }`, `{ "StructurallyEmergent": {} }`
- `confidence`: float (0.0–1.0)
- `epoch`: int (NSAI loop iteration)
- `iteration`: int
- `reasoning_depth`: int
- `extra`: map (usually empty)

**Edge type counts:**
| Type | Count |
|---|---|
| Affords | 1,002 |
| ActsOn | 285 |
| Modulates | 249 |
| ViaMechanism | 194 |
| PresentsIn | 58 |
| Amplifies | 3 |
| ContraindicatedWith | 3 |
| **Total** | **1,794** |

**Edge source breakdown:**
| Source | Count |
|---|---|
| Extracted | 1,072 |
| Deduced | 610 |
| StructurallyEmergent | 112 |

**Confidence distribution:**
- ≥ 0.7: 1,497 edges (83%)
- < 0.5: 251 edges (14%)
- All edges have a non-null confidence

**Sample:**
```json
{
  "id": "edge:abc123",
  "in": "node:curcumin",
  "out": "node:integumentary_system",
  "edge_type": { "ActsOn": {} },
  "metadata": {
    "source": { "Extracted": {} },
    "confidence": 1.0,
    "epoch": 0,
    "iteration": 1,
    "reasoning_depth": 0,
    "extra": {}
  }
}
```

---

### `edge_source`

```
Fields: id, source_node, target_node, edge_type (string), observation_type,
        confidence, model, provider, correlation_id, observed_at, source_tag
```

15,369 total rows. These are the raw LLM observations that produced or confirmed edges.

**Observation type breakdown:**
| Type | Count |
|---|---|
| confirmed | 13,575 |
| created | 1,794 |

The `created` rows correspond 1:1 with the 1,794 edges (one creation event per edge).
The `confirmed` rows are subsequent observations from different models that agreed.

**LLM provider breakdown:**
| Provider | Count |
|---|---|
| anthropic/claude-sonnet-4-6 | 5,961 |
| google/gemini-3-flash-preview | 4,951 |
| xai/grok-4-1-fast-reasoning | 3,847 |
| forward_chain | 610 |

(`forward_chain` = Deduced edges, not a real LLM provider)

**Sample:**
```json
{
  "id": "edge_source:000qxn631krgd5bqsm7f",
  "source_node": "nmn",
  "target_node": "heart strength",
  "edge_type": "affords",
  "observation_type": "confirmed",
  "confidence": 0.0,
  "model": "claude-sonnet-4-6",
  "provider": "anthropic",
  "correlation_id": "e0691e23-32ed-48a4-8624-5f199d9245fc",
  "observed_at": "2026-05-03T16:57:07.390159600+00:00",
  "source_tag": "Confirmed"
}
```

> **Note**: `edge_source.edge_type` is a plain string (`"affords"`, `"acts_on"`) while
> `edge.edge_type` is a tagged enum (`{ "Affords": {} }`). Different representations of
> the same concept — need to normalize during transfer.

---

### `node_source`

```
Fields: id, node_name, node_type (string), model, provider, correlation_id, observed_at
```

213 rows — exactly one per node. The creation provenance record for each node.

**Sample:**
```json
{
  "id": "node_source:0d14j2eb87z65ce7fbck",
  "node_name": "tart cherry",
  "node_type": "Ingredient",
  "model": "claude-sonnet-4-6",
  "provider": "anthropic",
  "correlation_id": "28420d8e-2107-4fa3-bc39-e4b7749bbce6",
  "observed_at": "2026-05-04T09:22:44.594142067+00:00"
}
```

---

### `edge_citation`

```
Fields: id, source_node, source_cui, target_node, target_cui,
        edge_type (string), suppkg_predicate, pmid, sentence, confidence
```

60,306 total rows. Three distinct populations (see SURREALDB_POSTGRES_TRANSFER.md for
full analysis). Summary:

**Population A — supplementology-confirmed (813 rows)**
- `suppkg_predicate = 'supplementology'`
- `edge_type` ∈ `{affords, acts_on, via_mechanism}` — NSAI types
- `sentence` = full PubMed abstract
- Real PMIDs from pubmed_trials
- `source_cui` and `target_cui` = empty

**Population A edge_type breakdown:**
| edge_type | Count |
|---|---|
| affords | 704 |
| acts_on | 95 |
| via_mechanism | 14 |

**Population B — SuppKG sentence matches (~40k with real PMIDs)**
- `suppkg_predicate` = uppercase SuppKG predicate (TREATS, AFFECTS, etc.)
- `sentence` = single sentence from paper
- `source_cui` / `target_cui` populated
- Full predicate breakdown:

| Predicate | Count | Keep? |
|---|---|---|
| PROCESS_OF | 5,193 | no — structural |
| TREATS | 5,082 | yes |
| COEXISTS_WITH | 4,696 | yes |
| AFFECTS | 4,606 | yes |
| PART_OF | 3,616 | no — structural |
| LOCATION_OF | 3,410 | no — structural |
| INTERACTS_WITH | 3,366 | yes |
| USES | 2,960 | no — structural |
| CAUSES | 2,044 | yes |
| ASSOCIATED_WITH | 1,997 | yes |
| PREVENTS | 1,995 | yes |
| AUGMENTS | 1,550 | yes |
| INHIBITS | 1,498 | yes |
| STIMULATES | 1,381 | yes |
| DISRUPTS | 1,202 | yes |
| ADMINISTERED_TO | 1,147 | no — methodological |
| COMPARED_WITH | 1,031+9 | no — methodological |
| PREDISPOSES | 664 | yes |
| ISA | 608 | no — structural |
| PRODUCES | 483 | yes |
| METHOD_OF | 432 | no — methodological |
| MEASURES | 145 | no — methodological |
| DIAGNOSES | 116 | no — methodological |
| PRECEDES | 98 | no — temporal |
| HIGHER_THAN | 81+1 | no — comparative |
| CONVERTS_TO | 65 | no — structural |
| COMPLICATES | 41 | no — clinical |
| OCCURS_IN | 25 | no — structural |
| INTERACTS_WITH(SPEC) | 9 | no — variant |
| TREATS(SPEC) | 9 | no — variant |
| TREATS(INFER) | 7 | no — variant |
| MANIFESTATION_OF | 4 | no — structural |
| SAME_AS | 4+1 | no — structural |
| PREVENTS(SPEC) | 2 | no — variant |
| PREDISPOSES(SPEC) | 1 | no — variant |
| CAUSES(SPEC) | 1 | no — variant |

**Population C — no-PMID rows (9,913)**
- `pmid = '0'` or `pmid = ''` — skip entirely

**Sample (Population B):**
```json
{
  "id": "edge_citation:000643lfxbd90plxekh3",
  "source_node": "supplements",
  "source_cui": "C0242295",
  "target_node": "autistic continuum",
  "target_cui": "C0524528",
  "edge_type": "TREATS",
  "suppkg_predicate": "TREATS",
  "pmid": "31351171",
  "sentence": "Metabolic interventions including special diets and supplements are commonly used in Autism Spectrum Disorder (ASD).",
  "confidence": 0.6697407365
}
```

---

### `ingredient_registry`

```
Fields: id, name, synonyms (array), search_terms (array),
        idisk_cui, idisk_id, ctd_mesh, suppkg_cui, umls_cui
```

19 rows. A small lookup cache used by the NSAI loop for ingredient resolution.
Contains CUI mappings that supplement the main graph. **Not a complete registry** —
the 97 Ingredient nodes in `node` are the authoritative ingredient list.

**Sample:**
```json
{
  "id": "ingredient_registry:0ihqrx2rqiffu1fl104w",
  "name": "vitamin c",
  "synonyms": ["ascorbate", "ascorbic acid", "sodium ascorbate"],
  "search_terms": ["vitamin c", "ascorbic acid"],
  "idisk_cui": "C2349136",
  "idisk_id": "DSI000041",
  "ctd_mesh": "",
  "suppkg_cui": "",
  "umls_cui": ""
}
```

Disposition: skip transfer — CUI data not needed in supplementology. Synonyms already
covered by supplementology's `synonym` table.

---

## Mapping Analysis: Where It Fits and Where It Doesn't

### ✅ Clean mappings

| SurrealDB | Postgres | Notes |
|---|---|---|
| `node` (Ingredient) | `entity` | 97 nodes → upsert to existing entities via name match |
| `node` (Symptom) | `entity` | 27 nodes → upsert to existing `symptom` entities |
| `edge_citation.sentence` | `evidence_claim.claim_text` | Direct copy |
| `edge_citation.confidence` | `evidence_claim.confidence` | Direct copy (cast float) |
| `edge_citation.pmid` | `citation.pmid` | Upsert by PMID |
| `edge_source` observation_type=confirmed | `evidence_claim` | LLM-confirmed edges as claims |
| `node_source` provenance | `evidence_claim` | Node creation provenance |

---

### ⚠️ Mapping friction points

**1. `edge.edge_type` — tagged enum vs. string**

In `edge`: `{ "Affords": {} }` (tagged enum, PascalCase)
In `edge_source`: `"affords"` (plain string, lowercase)
In `edge_citation`: `"TREATS"` (plain string, uppercase SuppKG)

The transfer script must normalize all three representations to a consistent form.
No data loss — just a serialization difference.

---

**2. NSAI node types Mechanism, Property, System have no supplementology entity_type**

| SurrealDB node_type | Count | Supplementology status |
|---|---|---|
| Mechanism | 37 | No `mechanism` entity_type exists |
| Property | 38 | No `property` entity_type exists |
| System | 14 | No `system` entity_type exists |

These concepts are NSAI loop semantic intermediaries — they were never designed to be
first-class KB citizens. Forcing them into `entity` requires three new Alembic-managed
entity_types. The right approach is Option C in the transfer plan: Alembic migration first,
then nodes, then edges. Layer 3 only.

---

**3. NSAI edges have two endpoints; `evidence_claim` has one (`entity_id`)**

An edge like `magnesium → Affords → muscle relaxation` has:
- `in`: node:magnesium (Ingredient → maps to entity)
- `out`: node:muscle_relaxation (Property → no entity yet)

The `evidence_claim` table has `entity_id` but no `target_entity_id`. Options:
- **Layer 2 workaround**: store target in `attrs` (no schema change, some information loss)
- **Layer 3 proper**: after Option C Alembic migration, use `relationship` for edges and
  `evidence_claim` for LLM observations backing them

Layer 2 (edge_citations) avoids this entirely — `target_node` goes into `attrs.target_node`.

---

**4. `edge_citation.source_node` — mixed case, needs synonym resolution**

176 distinct source_node values with mixed-case duplicates (e.g. `magnesium` and `Magnesium`).
Resolution test: 92/94 unique names resolve via `LOWER(synonym.name)` lookup.

Two failures:
- **`supplements`** (676 rows) — generic term, no entity, skip
- **`omega-3`** — canonical entity is "Omega-3 Fatty Acids"; no "omega-3" synonym exists.
  Fix: add "omega-3" as a synonym before running the Layer 2 transfer script.

---

**5. `edge_citation.sentence` for Population A is a full abstract, not a sentence**

Population A rows were created by confirm-edges, which stored the full PubMed abstract
in the `sentence` field (same field used for single sentences in Population B). This is
fine for `evidence_claim.claim_text` — full abstracts are valuable — but the field name
is misleading. No data loss; just worth knowing.

---

**6. `edge_citation` confidence dtype**

Stored as SurrealDB float: `0.6697407365f`. Python's `float()` handles this fine.
Population A rows from confirm-edges sometimes have `confidence = 0.699999988079071`
(float32 artifact). Round to 4 decimal places on insert.

---

**7. `edge_source` has no `sentence` / abstract text**

`edge_source` records LLM observations about edge existence — not citations. They do not
have a `pmid` or `sentence` field. When these become `evidence_claim` rows in Layer 3,
`citation_id` will be NULL (no citation backing, just LLM inference). The schema allows
nullable `citation_id`.

---

**8. SuppKG predicate case variants**

The predicate list includes `compared_with`, `higher_than`, `same_as` (lowercase) alongside
`COMPARED_WITH`, `HIGHER_THAN`, `SAME_AS` (uppercase). These are the same predicate.
Normalize to uppercase before applying the keep/skip filter.

---

**9. `node_source` — only one provenance record per node**

Each node has exactly one `node_source` record (created when the node was first generated).
No multi-model consensus records for nodes the way `edge_source` has for edges. When
converting to `evidence_claim`, these become single-provenance records with `source = 'nsai_graph'`.

---

### ❌ Skip — no transfer needed

| SurrealDB | Reason |
|---|---|
| `ingredient_registry` | Superseded by supplementology's `synonym` table; CUIs not needed |
| Population C (pmid='0'/'') | No citation backing; zero value |
| `edge_citation` structural predicates | PROCESS_OF, PART_OF, ISA, LOCATION_OF, etc. — not evidence |
| `node_alias` | Does not exist |

---

## Edge Type Semantics vs. Supplementology Rel Types

| SurrealDB | Proposed rel_type | Direction | Status in Postgres |
|---|---|---|---|
| Affords | `affords` | positive | Not yet in rel_type table |
| ActsOn | `acts_on` | neutral | Not yet in rel_type table |
| Modulates | `modulates` | neutral | Not yet in rel_type table |
| ViaMechanism | `via_mechanism` | neutral | Not yet in rel_type table |
| PresentsIn | `presents_in` | neutral | Not yet in rel_type table |
| Amplifies | `amplifies` | positive | Not yet in rel_type table |
| ContraindicatedWith | `contraindicated_with` | negative | Not yet in rel_type table |

All 7 are new rel_types. They coexist with existing types (`interacts_with`,
`has_adverse_reaction`, `is_effective_for`, etc.) via the `source` field.
These need an Alembic migration (Layer 3).

---

## Summary: Pre-Transfer Checklist

Before Layer 2 (edge_citations only — no schema changes):

- [ ] Add "omega-3" synonym to supplementology `synonym` table (entity: Omega-3 Fatty Acids)
- [ ] Verify supplementology `citation` and `evidence_claim` tables have no conflicting
      `source = 'supplementbot_confirmed'` rows from a prior partial run

Before Layer 3 (full NSAI graph — requires schema changes):

- [ ] Decision: do Mechanism, Property, System nodes belong in `entity`?
- [ ] If yes: Alembic migration to add `mechanism`, `property`, `system` entity_types
- [ ] Alembic migration to add 7 new rel_types
- [ ] Design slug scheme for Mechanism/Property/System nodes (e.g. `prop_muscle_relaxation`)
- [ ] Decide: does `evidence_claim` need `target_entity_id` (Option B) or do we use
      `relationship` + `evidence_claim` backing pattern (Option C)?

---

## Side-by-Side: supplementbot vs. supplementology

| Dimension | supplementbot SurrealDB | supplementology Postgres |
|---|---|---|
| Ingredient coverage | 97 nodes | 97 seeded entities (same set) |
| Non-ingredient concepts | 116 (Mechanism/Property/System/Symptom) | 27 Symptoms already exist; others absent |
| Relationships | 1,794 NSAI edges (Affords, ActsOn, etc.) | 3,679 rows (is_effective_for, interacts_with, etc.) — different semantics |
| Relationship typing | 7 NSAI semantic types | 4 pharmacological types |
| Citation evidence | 60,306 edge_citations (3 populations) | 40,933 evidence_claims (6 sources) |
| Citation metadata | PMID + sentence only | Full title, authors, journal, year, study_type |
| LLM provenance | 15,369 edge_source observations | Not tracked (no equivalent table) |
| Node provenance | 213 node_source records | Not tracked |
| Confidence scoring | On every edge (0.0–1.0) | On most evidence_claims and relationships |
| Entity resolution | Name-based (NSAI loop internal) | Synonym table with DSLD product counts |
| CUI mappings | Some in ingredient_registry | Not in supplementology |
