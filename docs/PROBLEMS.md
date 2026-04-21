# Citation Problem

## The Goal

When the chat UI mentions a supplement (e.g., "Quercetin may help with seasonal allergies"), a
citations panel should open showing PubMed-backed evidence for that supplement. The user sees:
a supporting sentence from a paper, a PubMed link, and a confidence score.

---

## What We Have

### SuppKG (`data/suppkg/supp_kg.json`)

A knowledge graph of ~500k supplement-concept edges. Each edge carries PubMed citations (PMID,
supporting sentence, confidence). 1.2 million real PMIDs total. This is our only source of
PubMed citations.

SuppKG nodes are identified by CUI — a mix of:
- **UMLS `C*` CUIs** — real Unified Medical Language System identifiers (e.g., `C1268858`)
- **Internal `DC*` CUIs** — SuppKG-specific identifiers with no external equivalent (e.g., `DC0003968`)

SuppKG also has a **term index**: a mapping from human-readable names to CUIs. This is used
to look up "magnesium" → `C1268858`.

### The Training Pipeline

During training (`cargo run --bin supplementbot -- train`), `run_citation_backing()` in
`crates/nsai-loop/src/citations.rs`:

1. For each Ingredient node in the graph, resolves its CUI via:
   - Hardcoded overrides (6 ingredients)
   - Merge store (populated by `--resolve-cuis`)
   - SuppKG term index
2. Looks up all SuppKG edges for that CUI
3. Stores citations into the `edge_citation` SurrealDB table keyed by **ingredient name**

At query time, `source_store.citations_for_ingredient("magnesium")` retrieves them by name.
No CUI lookup needed at query time.

### Our 19 Ingredients

The graph currently has these Ingredient nodes:
magnesium, quercetin, zinc, vitamin d, vitamin c, ashwagandha, probiotics, coq10, nac,
rhodiola rosea, vitamin b complex, alpha-lipoic acid, melatonin, fish oil, turmeric,
iron, calcium, gaba, theanine

---

## The Core Problem

**SuppKG's term index does not reliably match our ingredient names.**

Coverage breakdown:
- **Hardcoded CUI overrides** (citations.rs): magnesium, zinc, vitamin d, vitamin c, berberine,
  omega-3/fish oil — 6 ingredients. These work because we manually found the right CUI.
- **SuppKG term index matches**: A few more via exact name match (e.g., turmeric → curcumin node)
- **No match at all**: ashwagandha, probiotics, coq10, rhodiola rosea, vitamin b complex,
  alpha-lipoic acid, nac, gaba, quercetin, theanine — roughly 10-13 of 19

### Why Term Index Matching Fails

SuppKG's term list uses pharmaceutical/clinical names, not consumer supplement names:
- "CoQ10" → SuppKG has "ubiquinone" and "ubiquinol" as separate nodes with different CUIs
- "NAC" → SuppKG has "N-acetyl-L-cysteine" but also matches unrelated compounds
- "alpha-lipoic acid" → SuppKG term index may have it under "thioctic acid" or similar
- "ashwagandha" → may not exist at all in SuppKG (no node)
- "probiotics" → too generic; SuppKG has strain-specific nodes (Lactobacillus acidophilus, etc.)

### Why UMLS API Didn't Help

We built `crates/umls-client/` and ran `--resolve-cuis` to populate `supplement_cuis.jsonl`
with UMLS `C*` CUIs for 15/19 ingredients. Then checked whether those CUIs exist in SuppKG.

**Result: they mostly don't.** The UMLS API returns the canonical UMLS CUI for a concept —
but SuppKG was built from a specific subset of UMLS and uses its own `DC*` namespace for many
nodes. The `C*` CUIs returned by the UMLS API are a *different set* from the `C*` CUIs
actually present in SuppKG.

Example: UMLS API returns `C0522062` for quercetin. SuppKG may have quercetin under a `DC*`
CUI, or under `C0522062` with different edge coverage, or not at all. We verified that for
most of our ingredients, the UMLS API CUI ≠ the SuppKG CUI that has good edge coverage.

### Why iDISK Didn't Help

iDISK 2.0 (`data/idisk2/`) has UMLS CUIs for 16/19 of our ingredients (DSI.csv: iDISK_ID,
Name, CUI). However:
- iDISK does **not** contain PubMed PMIDs. The `Source` column in relation files says "MSKCC"
  — attribution to a database, not an actual paper.
- iDISK's UMLS CUIs have the same mismatch problem as above against SuppKG.
- iDISK is useful for ingredient→CUI mapping but not as a citation source.

### Why SemMedDB Was Ruled Out

SemMedDB was a large NLP-extracted biomedical KG with PubMed citations. It was
**deprecated December 31, 2024** by NLM. Do not pursue this path.

---

## What We've Tried (Chronological)

1. **Hardcoded CUI table** in `citations.rs` — covers 6 ingredients only, doesn't scale
2. **UMLS API + `supplement_cuis.jsonl`** — resolves CUIs but they don't match SuppKG nodes
3. **Fuzzy SuppKG term matching** — considered and rejected; too many false positives for
   generic terms like "zinc" (matches zinc stearate, zinc oxide, etc.)
4. **DrugCentral SMILES file** — checked for CUI mapping; DrugCentral uses integer IDs,
   SuppKG's `DC*` prefix is unrelated to DrugCentral. Dead end.
5. **SemMedDB** — deprecated December 2024. Dead end.
6. **iDISK citations** — iDISK has no PMIDs. Dead end for citations.
7. **iDISK CUIs → SuppKG** — same UMLS CUI mismatch problem. Dead end.

---

## Current State of the Code

The handler (`crates/web-server/src/handler.rs`) was recently fixed to query `edge_citation`
by ingredient name directly via `source_store.citations_for_ingredient()`, instead of doing
a CUI lookup at chat time. **This part is correct.**

The broken part is upstream: `run_citation_backing()` during training cannot find SuppKG nodes
for ~10-13 of our 19 ingredients, so those rows never get written to `edge_citation` in the
first place. The chat-time query correctly finds nothing because nothing was stored.

---

## Possible Paths Forward

### Path A: Better SuppKG Name Matching (Low Effort, Uncertain Payoff)

Try harder to match our ingredient names to SuppKG nodes at training time:
- Try multiple name variants per ingredient (e.g., "coq10", "coenzyme q10", "ubiquinone")
- Try SuppKG synonym fields
- Try partial/word-level matching with disambiguation

**Risk**: "probiotics" and "vitamin b complex" have no single SuppKG node — they're categories.
"GABA" is ambiguous (neurotransmitter vs. supplement). Fuzzy matching for these will produce
wrong CUIs with high-confidence-looking edges.

### Path B: Replace SuppKG with a KG That Uses Standard UMLS CUIs (High Effort, Best Long-Term)

Find a knowledge graph that:
1. Has edges between supplement/drug concepts and clinical outcomes
2. Carries PubMed citations (PMID + sentence)
3. Uses standard UMLS `C*` CUIs that match what the UMLS API returns
4. Is not deprecated
5. Covers consumer supplement names (ashwagandha, probiotics, etc.)

Candidates to investigate (none confirmed viable):
- **PKG (Pharmacological Knowledge Graph)** — unclear if it has PMID-level citations
- **PrimeKG** — drug-disease KG, unclear supplement coverage
- **Hetionet** — uses UMLS-compatible identifiers, covers drugs/diseases, unclear supplements
- **NLM's Indexing Initiative GitHub** (SemMedDB's intended replacement) — needs investigation
- **OpenKE / Bio2RDF supplement subgraph** — needs investigation

### Path C: Build Our Own Citation Index from PubMed (High Effort, Best Coverage)

Use PubMed's E-utilities API (free, no quota abuse) to:
1. For each ingredient, fetch relevant PMIDs (search by ingredient name + supplement terms)
2. Fetch abstracts for top N papers
3. Extract supporting sentences (NLP or LLM-based)
4. Store in `edge_citation` with our ingredient name as key

This bypasses the KG namespace problem entirely. We control the ingredient→citation mapping.
Downside: no pre-built edge structure — we'd be building relationships from scratch.

### Path D: Use iDISK Source Descriptions as Pseudo-Citations (Low Effort, Weak Evidence)

iDISK's `Background`, `Safety`, and `Mechanism of action` columns contain paragraph text with
embedded reference numbers like "(34)" that reference a bibliography. The actual bibliography
is not included in the CSV files.

If we could find the source bibliographies (MSKCC, NMCD) and cross-reference the embedded
citation numbers, we could extract real PMIDs. This is indirect and fragile.

---

## Open Questions for External Research

1. Is there a current (non-deprecated) biomedical knowledge graph that:
   - Has UMLS `C*` CUI-identified nodes for common dietary supplements
   - Carries PubMed PMIDs on its edges
   - Covers consumer supplement names like ashwagandha, rhodiola rosea, probiotics?

2. Does PKG 2.0 or any successor to SemMedDB provide downloadable RDF/JSON with PMID citations?

3. Does the iDISK 2.0 GitHub repository include the source bibliographies that the embedded
   reference numbers in DSI.csv point to?

4. Is there a PubMed-derived supplement-specific citation database (like what MSKCC uses
   internally) that is publicly downloadable?
