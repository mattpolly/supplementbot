# External Data Resources for Supplement Information

Last updated: 2026-04-21

## Purpose

When the citation pipeline can't find a relevant PubMed citation for a specific symptom, we need
a fallback: general-purpose ingredient summaries (what it is, what it's known for, mechanisms,
safety). This document catalogs every external data source we evaluated, what it contains, its
license, and whether it's practically usable.

---

## Tier 1: Public Domain, Real APIs, Usable Today

### NIH Office of Dietary Supplements (ODS) Fact Sheets

- **URL**: https://ods.od.nih.gov/factsheets/list-all/
- **API**: `https://ods.od.nih.gov/api/index.aspx?resourcename={Name}&readinglevel={Level}&outputformat=XML`
- **License**: US government work — **public domain**, no attribution required
- **Format**: XML via REST API, no auth needed. Reading levels: `Consumer`, `HealthProfessional`
- **Content**: Expert-reviewed monographs covering: introduction, "what the science says," safety
  concerns, drug interactions, dosing, and references. Very high quality.
- **Coverage of our 19 ingredients**:
  - FOUND: ashwagandha, probiotics, magnesium, vitamin D, omega-3/fish oil, vitamin C, zinc,
    iron, calcium, melatonin, vitamin B complex (partial — individual B vitamins have sheets)
  - NOT FOUND: quercetin, CoQ10, NAC, rhodiola, GABA, theanine, turmeric/curcumin,
    alpha-lipoic acid
- **Notes**: Resource names must match exactly (e.g., "Ashwagandha" not "ashwagandha"). Covers
  ~50-60 supplements total, weighted toward vitamins, minerals, and popular botanicals.
- **Verdict**: Must-use for every ingredient it covers. Public domain means we can store and
  display the text freely.

### PubChem PUG-View API

- **URL**: https://pubchem.ncbi.nlm.nih.gov/
- **API**:
  - Properties: `https://pubchem.ncbi.nlm.nih.gov/rest/pug/compound/name/{name}/property/{props}/JSON`
  - Pharmacology: `https://pubchem.ncbi.nlm.nih.gov/rest/pug_view/data/compound/{CID}/JSON?heading=Pharmacology+and+Biochemistry`
- **License**: US government work — **public domain**
- **Format**: REST JSON, no auth needed
- **Content**: Pharmacological classification, mechanism of action (multiple entries per compound),
  absorption/distribution/metabolism/excretion (ADME), biological half-life, toxicity, safety,
  drug interactions. All structured under named headings.
- **Coverage of our 19 ingredients**:
  - FOUND: quercetin (CID 5280343), NAC (CID 12035), theanine (CID 439378), GABA (CID 119),
    CoQ10/ubiquinone (CID 5281915), ubiquinol (CID 9962735), alpha-lipoic acid, melatonin,
    magnesium (as element), zinc (as element), iron (as element), calcium (as element),
    vitamin C, vitamin D, salidroside/rhodiola active compound (CID 159278)
  - NOT DIRECTLY FOUND: ashwagandha (a plant, not a single molecule — would need to search for
    withanolides), probiotics (organisms, not compounds), fish oil (mixture), turmeric (search
    for curcumin CID 969516 instead)
- **Notes**: Works best for pure chemical compounds. For botanicals, search by the active compound
  name instead (curcumin for turmeric, withanolide A for ashwagandha, salidroside for rhodiola).
  The pharmacology data is sourced from DrugBank, HSDB, and other authoritative databases.
- **Verdict**: Excellent for compounds. Complements NIH ODS — ODS covers the botanicals and
  categories that PubChem misses, PubChem covers the pure compounds that ODS misses.

### NIH DSLD (Dietary Supplement Label Database)

- **URL**: https://dsld.od.nih.gov/
- **API**: `https://api.ods.od.nih.gov/dsld/v9/`
  - Search: `/v9/search-filter?q={term}`
  - Ingredient groups: `/v9/ingredient-groups?term={term}&method=contains`
  - Product label: `/v9/label/{id}`
- **License**: US government work — **public domain**
- **Format**: REST JSON, no auth needed
- **Content**: Label data from 200,000+ supplement products: product names, brand names,
  ingredient lists with amounts/forms, daily value percentages, health claims, warning
  statements, supplement form (capsule/tablet/etc).
- **Coverage**: Very broad for products — virtually every supplement sold in the US is represented.
- **Notes**: This is **product label data**, not ingredient knowledge. You get "Quercetin 500mg
  per capsule by Brand X" but not "quercetin is a flavonoid antioxidant." Useful for understanding
  common forms, doses, and manufacturer claims, but not for ingredient descriptions or mechanisms.
- **Verdict**: Not useful for the general-info fallback use case. Potentially useful later for
  product-level features (dose recommendations, form comparisons).

---

## Tier 2: Open License, Requires Attribution

### Wikipedia (MediaWiki API)

- **URL**: https://en.wikipedia.org/
- **API**:
  - Summary: `https://en.wikipedia.org/api/rest_v1/page/summary/{title}`
  - Section wikitext: `https://en.wikipedia.org/w/api.php?action=parse&page={title}&prop=wikitext&section={n}`
  - Table of contents: `https://en.wikipedia.org/w/api.php?action=parse&page={title}&prop=tocdata`
- **License**: **CC BY-SA 3.0/4.0** — commercial use allowed with attribution and share-alike
- **Format**: REST API, no auth needed. Returns JSON with wikitext or HTML.
- **Content**: Rich prose covering descriptions, traditional uses, phytochemistry, mechanisms of
  action, clinical research, safety/side effects, and dosing. Typically well-referenced with
  PubMed citations in the article footnotes.
- **Coverage of our 19 ingredients**: All 19 have Wikipedia articles. Some redirect to the
  botanical/chemical entity (ashwagandha → "Withania somnifera", CoQ10 → "Coenzyme Q10").
- **Notes**:
  - Section names vary between articles — no consistent structure. Extraction requires
    per-article mapping or heuristic section matching.
  - Articles are organized around the botanical/chemical entity, not "supplement use."
  - Python `wikipedia-api` package simplifies extraction.
  - Attribution is required — must credit Wikipedia as source when displaying content.
- **Verdict**: Best free source for prose descriptions covering all ingredient types. The universal
  fallback when ODS and PubChem don't have coverage. Use an LLM to summarize/standardize the
  extracted sections into a consistent format.

### Wikidata (SPARQL)

- **URL**: https://www.wikidata.org/
- **API**: SPARQL endpoint at `https://query.wikidata.org/sparql`
- **License**: **CC0** — public domain dedication, no attribution required
- **Format**: SPARQL queries returning JSON/XML
- **Content**: Chemical identifiers (CAS, SMILES, InChI, MeSH ID, ChEMBL ID, UNII), chemical
  class/subclass, "found in taxon" (plant sources), molecular formula, cross-references to
  PubChem/PDB/pharmacology databases. Short one-line descriptions only.
- **Coverage**: Found quercetin, theanine, magnesium as chemical entities. Ashwagandha exists as
  a plant entity. The `P2175` ("medical condition treated") property exists but was empty for
  tested supplements.
- **Notes**: Primarily useful as a cross-reference hub linking identifiers across databases,
  not as a content source. One-line descriptions are too brief for our fallback use case.
- **Verdict**: Useful for building the ingredient registry's cross-reference IDs (CAS → PubChem
  CID → MeSH ID → UNII). Not useful for ingredient descriptions or summaries.

---

## Tier 3: Paywalled or License Concerns

### Natural Medicines (NatMed Pro, formerly NMCD)

- **URL**: https://naturalmedicines.therapeuticresearch.com/
- **License**: **Paywalled** — subscriptions start at $69/year individual
- **Content**: Clinical monographs on ~90,000 products. Efficacy ratings, safety, interactions,
  dosing, mechanisms. The gold standard for clinical supplement data.
- **Verdict**: Not usable for an open project without a licensing agreement.

### ConsumerLab

- **URL**: https://www.consumerlab.com/
- **License**: **Paywalled** — subscription required
- **Content**: Product testing and quality reviews, not ingredient knowledge.
- **Verdict**: Not usable. Tests supplement products for quality/purity, not what we need.

### HerbMed / HerbMedPro

- **URL**: https://www.herbmed.org/
- **License**: HerbMed (basic) is free but web-only. HerbMedPro requires subscription.
- **Content**: Curated PubMed links organized by herb. Free version is essentially a bibliography.
- **Format**: Web-only SPA, no API. Would require scraping.
- **Verdict**: Limited utility. Basically curated PubMed links without structured ingredient data.

### iDISK 2.0 (Integrated Dietary Supplements Knowledge Base)

- **URL**: https://github.com/houyurain/iDISK2.0
- **License**: Available on GitHub but aggregates data from paywalled sources (NMCD, Memorial
  Sloan Kettering). **Commercial use rights uncertain** without legal review.
- **Content**: Structured KG with dietary supplement ingredients, products, diseases, drugs,
  symptoms, and their relationships. 7,876 DSI entries. Rich metadata including background text,
  mechanism of action, safety paragraphs.
- **Format**: Neo4j graph dump and CSV/RRF files.
- **Notes**: We already use iDISK for synonym resolution and CUI mapping (see PROBLEMS.md). The
  descriptive text fields (Background, Mechanism of Action, Safety) would be ideal for our
  fallback summaries, but the license situation makes this risky for user-facing display.
- **Verdict**: Continue using for internal synonym/ID mapping. Do NOT use the descriptive text
  for user-facing display without legal review of the underlying source licenses.

### FooDB (Food Database)

- **URL**: https://foodb.ca/
- **License**: **CC BY-NC 4.0** — non-commercial use only
- **Content**: 28,000+ chemical compounds in 1,000+ foods. Chemical properties, biological
  activities, dietary sources, health effects.
- **Coverage**: Food-derived compounds only (quercetin yes, ashwagandha no, probiotics no).
- **Verdict**: Non-commercial license is a constraint. Limited supplement coverage. Not useful.

---

## Tier 4: Evaluated and Ruled Out

### NIH DSID (Dietary Supplement Ingredient Database)

- **URL**: https://data.nal.usda.gov/dataset/dietary-supplement-ingredient-database-dsid-release-40
- **Content**: Analytically verified ingredient amounts vs. labeled amounts. Answers "does this
  product actually contain what it says?" not "what does this ingredient do?"
- **Verdict**: Not relevant for ingredient descriptions. Quality assurance data only.

### NCCIH Herbs at a Glance

- **URL**: https://www.nccih.nih.gov/health/herbsataglance
- **License**: US government work — public domain
- **Content**: Brief fact sheets: common names, what the science says, side effects/cautions.
  Covers 50+ herbs.
- **Format**: **Web pages only, no API.** Would require HTML scraping.
- **Notes**: Good content but no programmatic access. The content overlaps significantly with
  NIH ODS fact sheets. The HerbList mobile app uses this data but has no open API.
- **Verdict**: Worth scraping as a supplement to ODS if we need broader botanical coverage, but
  lower priority since ODS already covers the high-value items and has a real API.

### Examine.com

- **URL**: https://examine.com/
- **Content**: Detailed supplement monographs with research summaries.
- **API**: None available as of 2026-04-21.
- **License**: Proprietary content.
- **Verdict**: No API, proprietary. Not usable.

### Open Food Facts

- **URL**: https://world.openfoodfacts.org/data
- **License**: ODbL — open for commercial use
- **Content**: Consumer food product data including some supplement products. Ingredient lists,
  not ingredient knowledge.
- **Verdict**: Not relevant for supplement ingredient information.

---

## Recommended Multi-Source Strategy

### Coverage Matrix

| Ingredient | NIH ODS | PubChem | Wikipedia | Best Source |
|---|---|---|---|---|
| magnesium | Yes | Yes (element) | Yes | ODS |
| zinc | Yes | Yes (element) | Yes | ODS |
| vitamin D | Yes | Yes | Yes | ODS |
| vitamin C | Yes | Yes | Yes | ODS |
| iron | Yes | Yes (element) | Yes | ODS |
| calcium | Yes | Yes (element) | Yes | ODS |
| fish oil | Yes (omega-3) | No (mixture) | Yes | ODS |
| probiotics | Yes | No (organisms) | Yes | ODS |
| melatonin | Yes | Yes | Yes | ODS |
| vitamin B complex | Partial | Partial | Yes | ODS + Wikipedia |
| ashwagandha | Yes | No (botanical) | Yes | ODS |
| quercetin | No | Yes (CID 5280343) | Yes | PubChem |
| CoQ10 | No | Yes (CID 5281915) | Yes | PubChem |
| NAC | No | Yes (CID 12035) | Yes | PubChem |
| theanine | No | Yes (CID 439378) | Yes | PubChem |
| GABA | No | Yes (CID 119) | Yes | PubChem |
| alpha-lipoic acid | No | Yes | Yes | PubChem |
| turmeric | No | Yes (curcumin CID 969516) | Yes | PubChem |
| rhodiola rosea | No | Partial (salidroside) | Yes | Wikipedia |

### Fetch Priority Per Ingredient Type

| Ingredient type | Primary (public domain) | Fallback (CC BY-SA) |
|---|---|---|
| Vitamins & minerals | NIH ODS | Wikipedia |
| Pure compounds (quercetin, NAC, theanine, GABA, CoQ10, ALA) | PubChem | Wikipedia |
| Botanicals (ashwagandha, rhodiola, turmeric) | NIH ODS if available | Wikipedia |
| Categories (probiotics, vitamin B complex) | NIH ODS | Wikipedia |
| Everything else (future ingredients) | Try ODS → PubChem → Wikipedia | — |

### Implementation Notes

- **Fetch at training time**, not query time. Store summaries in a DB table keyed by ingredient
  name, same pattern as `edge_citation`.
- **LLM summarization**: Raw Wikipedia articles and PubChem pharmacology sections vary wildly in
  length and structure. Use an LLM to distill each into a consistent 2-4 sentence summary
  covering: what it is, primary mechanisms, and what it's commonly used for.
- **Attribution**: NIH ODS and PubChem are public domain (no attribution needed). Wikipedia
  content requires CC BY-SA attribution — display "Source: Wikipedia" with a link when showing
  Wikipedia-derived summaries.
- **Tiered display in chat UI**:
  1. Relevant PubMed citation (keyword match against session context) → show citation
  2. No relevant citation → show general ingredient summary as fallback
  3. Optionally blend: summary at top, best available citations below
- **Wikidata for ID cross-referencing**: Use SPARQL to populate the ingredient registry with
  CAS numbers, PubChem CIDs, MeSH IDs, and UNII codes. This helps with future source
  integration.
