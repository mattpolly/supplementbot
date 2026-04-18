// ---------------------------------------------------------------------------
// iDISK 2.0 Importer
//
// Loads iDISK entity and relation CSV files into the intake graph:
//   - SS.csv → SymptomProfile nodes (with CUI and MSKCC aliases)
//   - D.csv → Drug entities (for interaction checking)
//   - DSI.csv → Ingredient enrichment (Mechanism of Action, Safety text)
//   - dsi_ss.csv → has_adverse_reaction edges
//   - dsi_d.csv → interacts_with edges (with descriptions)
//   - dsi_dis.csv → is_effective_for edges
//
// All entities keyed by iDISK_ID. UMLS CUIs stored for cross-source resolution.
// ---------------------------------------------------------------------------

use std::collections::HashMap;
use std::path::Path;

use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use surrealdb_types::{RecordId, SurrealValue};

use super::store::IntakeGraphStore;
use super::types::*;

// ---------------------------------------------------------------------------
// iDISK-specific DB records (separate from intake graph nodes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, SurrealValue)]
struct IdiskDrugRecord {
    idisk_id: String,
    name: String,
    cui: Option<String>,
    aliases: String, // JSON array
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskDrugRecordWithId {
    id: RecordId,
    idisk_id: String,
    name: String,
    cui: Option<String>,
    aliases: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskIngredientRecord {
    idisk_id: String,
    name: String,
    cui: Option<String>,
    mechanism_of_action: Option<String>,
    safety: Option<String>,
    background: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskIngredientRecordWithId {
    id: RecordId,
    idisk_id: String,
    name: String,
    cui: Option<String>,
    mechanism_of_action: Option<String>,
    safety: Option<String>,
    background: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskAdverseReactionRecord {
    ingredient_id: String,
    symptom_id: String,
    source: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskAdverseReactionRecordWithId {
    id: RecordId,
    ingredient_id: String,
    symptom_id: String,
    source: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskInteractionRecord {
    ingredient_id: String,
    drug_id: String,
    source: String,
    interaction_rating: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskInteractionRecordWithId {
    id: RecordId,
    ingredient_id: String,
    drug_id: String,
    source: String,
    interaction_rating: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskEffectivenessRecord {
    ingredient_id: String,
    disease_id: String,
    disease_name: String,
    source: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct IdiskEffectivenessRecordWithId {
    id: RecordId,
    ingredient_id: String,
    disease_id: String,
    disease_name: String,
    source: String,
}

// ---------------------------------------------------------------------------
// Importer
// ---------------------------------------------------------------------------

pub struct IdiskImporter {
    db: Surreal<Any>,
}

/// Import stats returned after loading.
#[derive(Debug, Default)]
pub struct ImportStats {
    pub symptom_profiles: usize,
    pub drugs: usize,
    pub ingredients: usize,
    pub adverse_reactions: usize,
    pub interactions: usize,
    pub effectiveness: usize,
}

impl IdiskImporter {
    pub fn new(db: &Surreal<Any>) -> Self {
        Self { db: db.clone() }
    }

    /// Import all iDISK data from the given directory.
    /// Expects the standard iDISK 2.0 directory structure:
    ///   data_dir/Entity/SS.csv, D.csv, DSI.csv, Dis.csv
    ///   data_dir/Relation/dsi_ss.csv, dsi_d.csv, dsi_dis.csv
    pub async fn import_all(
        &self,
        data_dir: &Path,
        intake_store: &IntakeGraphStore,
    ) -> Result<ImportStats, String> {
        let mut stats = ImportStats::default();

        // Load entity name maps for resolving IDs in relation files
        let ss_names = self
            .import_symptoms(
                &data_dir.join("Entity/SS.csv"),
                intake_store,
                &mut stats,
            )
            .await?;

        let drug_names = self
            .import_drugs(&data_dir.join("Entity/D.csv"), &mut stats)
            .await?;

        let dsi_names = self
            .import_ingredients(&data_dir.join("Entity/DSI.csv"), &mut stats)
            .await?;

        let dis_names = self
            .load_disease_names(&data_dir.join("Entity/Dis.csv"))
            .await?;

        // Import relations
        self.import_adverse_reactions(
            &data_dir.join("Relation/dsi_ss.csv"),
            &dsi_names,
            &ss_names,
            &mut stats,
        )
        .await?;

        self.import_interactions(
            &data_dir.join("Relation/dsi_d.csv"),
            &dsi_names,
            &drug_names,
            &mut stats,
        )
        .await?;

        self.import_effectiveness(
            &data_dir.join("Relation/dsi_dis.csv"),
            &dsi_names,
            &dis_names,
            &mut stats,
        )
        .await?;

        Ok(stats)
    }

    // -----------------------------------------------------------------------
    // Entity importers
    // -----------------------------------------------------------------------

    /// Import SS.csv → SymptomProfile nodes.
    /// Returns a map of iDISK_ID → symptom name for relation resolution.
    async fn import_symptoms(
        &self,
        path: &Path,
        intake_store: &IntakeGraphStore,
        stats: &mut ImportStats,
    ) -> Result<HashMap<String, String>, String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        let mut id_to_name = HashMap::new();

        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;

            let idisk_id = record.get(0).unwrap_or("").to_string();
            let name = record.get(1).unwrap_or("").to_string();
            let cui = record.get(2).map(|s| s.to_string()).filter(|s| !s.is_empty());
            let mskcc_aliases_raw = record.get(3).unwrap_or("");

            // Parse MSKCC aliases (pipe-separated)
            let aliases: Vec<String> = mskcc_aliases_raw
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s.to_lowercase() != name.to_lowercase())
                .collect();

            let slug_id = slug(&name);
            id_to_name.insert(idisk_id, name.clone());

            // Determine archetype from symptom name heuristics
            let archetype = guess_archetype(&name, &aliases);

            intake_store
                .add_symptom_profile(&SymptomProfile {
                    id: slug_id,
                    name: name.clone(),
                    cui,
                    aliases,
                    archetype_id: archetype,
                    relevant_oldcarts_override: None,
                    irrelevant_oldcarts_override: None,
                    sufficient_dimensions_override: None,
                    associated_systems: vec![],
                })
                .await;

            stats.symptom_profiles += 1;
        }

        Ok(id_to_name)
    }

    /// Import D.csv → idisk_drug table.
    async fn import_drugs(
        &self,
        path: &Path,
        stats: &mut ImportStats,
    ) -> Result<HashMap<String, String>, String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        let mut id_to_name = HashMap::new();

        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;

            let idisk_id = record.get(0).unwrap_or("").to_string();
            let name = record.get(1).unwrap_or("").to_string();
            let cui = record.get(2).map(|s| s.to_string()).filter(|s| !s.is_empty());
            let mskcc_raw = record.get(3).unwrap_or("");

            let aliases: Vec<String> = mskcc_raw
                .split('|')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let slug_id = slug(&name);
            id_to_name.insert(idisk_id.clone(), name.clone());

            let _: Option<IdiskDrugRecordWithId> = self
                .db
                .create(("idisk_drug", slug_id.as_str()))
                .content(IdiskDrugRecord {
                    idisk_id,
                    name,
                    cui,
                    aliases: serde_json::to_string(&aliases).unwrap_or_default(),
                })
                .await
                .unwrap_or(None);

            stats.drugs += 1;
        }

        Ok(id_to_name)
    }

    /// Import DSI.csv → idisk_ingredient table (Mechanism of Action, Safety, Background).
    async fn import_ingredients(
        &self,
        path: &Path,
        stats: &mut ImportStats,
    ) -> Result<HashMap<String, String>, String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        let mut id_to_name = HashMap::new();

        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;

            let idisk_id = record.get(0).unwrap_or("").to_string();
            let name = record.get(1).unwrap_or("").to_string();
            let cui = record.get(2).map(|s| s.to_string()).filter(|s| !s.is_empty());
            let background = record.get(3).map(|s| s.to_string()).filter(|s| !s.is_empty());
            let safety = record.get(4).map(|s| s.to_string()).filter(|s| !s.is_empty());
            let mechanism = record.get(5).map(|s| s.to_string()).filter(|s| !s.is_empty());

            let slug_id = slug(&name);
            id_to_name.insert(idisk_id.clone(), name.clone());

            let _: Option<IdiskIngredientRecordWithId> = self
                .db
                .create(("idisk_ingredient", slug_id.as_str()))
                .content(IdiskIngredientRecord {
                    idisk_id,
                    name,
                    cui,
                    mechanism_of_action: mechanism,
                    safety,
                    background,
                })
                .await
                .unwrap_or(None);

            stats.ingredients += 1;
        }

        Ok(id_to_name)
    }

    /// Load Dis.csv names for relation resolution (not stored as nodes).
    async fn load_disease_names(
        &self,
        path: &Path,
    ) -> Result<HashMap<String, String>, String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        let mut id_to_name = HashMap::new();
        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;
            let idisk_id = record.get(0).unwrap_or("").to_string();
            let name = record.get(1).unwrap_or("").to_string();
            id_to_name.insert(idisk_id, name);
        }
        Ok(id_to_name)
    }

    // -----------------------------------------------------------------------
    // Relation importers
    // -----------------------------------------------------------------------

    /// Import dsi_ss.csv → idisk_adverse_reaction table.
    async fn import_adverse_reactions(
        &self,
        path: &Path,
        dsi_names: &HashMap<String, String>,
        ss_names: &HashMap<String, String>,
        stats: &mut ImportStats,
    ) -> Result<(), String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;

            let dsi_id = record.get(0).unwrap_or("").to_string();
            let ss_id = record.get(2).unwrap_or("").to_string();
            let source = record.get(3).unwrap_or("").to_string();

            let ingredient_name = dsi_names.get(&dsi_id).cloned().unwrap_or(dsi_id.clone());
            let symptom_name = ss_names.get(&ss_id).cloned().unwrap_or(ss_id.clone());

            let key = format!("{}_{}", slug(&ingredient_name), slug(&symptom_name));

            let _: Option<IdiskAdverseReactionRecordWithId> = self
                .db
                .create(("idisk_adverse_reaction", key.as_str()))
                .content(IdiskAdverseReactionRecord {
                    ingredient_id: slug(&ingredient_name),
                    symptom_id: slug(&symptom_name),
                    source,
                })
                .await
                .unwrap_or(None);

            stats.adverse_reactions += 1;
        }

        Ok(())
    }

    /// Import dsi_d.csv → idisk_interaction table (with descriptions).
    async fn import_interactions(
        &self,
        path: &Path,
        dsi_names: &HashMap<String, String>,
        drug_names: &HashMap<String, String>,
        stats: &mut ImportStats,
    ) -> Result<(), String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;

            let dsi_id = record.get(0).unwrap_or("").to_string();
            let drug_id = record.get(2).unwrap_or("").to_string();
            let source = record.get(3).unwrap_or("").to_string();
            let rating = record.get(4).map(|s| s.to_string()).filter(|s| !s.is_empty());
            let description = record.get(5).map(|s| s.to_string()).filter(|s| !s.is_empty());

            let ingredient_name = dsi_names.get(&dsi_id).cloned().unwrap_or(dsi_id.clone());
            let drug_name = drug_names.get(&drug_id).cloned().unwrap_or(drug_id.clone());

            let key = format!("{}_{}", slug(&ingredient_name), slug(&drug_name));

            let _: Option<IdiskInteractionRecordWithId> = self
                .db
                .create(("idisk_interaction", key.as_str()))
                .content(IdiskInteractionRecord {
                    ingredient_id: slug(&ingredient_name),
                    drug_id: slug(&drug_name),
                    source,
                    interaction_rating: rating,
                    description,
                })
                .await
                .unwrap_or(None);

            stats.interactions += 1;
        }

        Ok(())
    }

    /// Import dsi_dis.csv → idisk_effectiveness table.
    async fn import_effectiveness(
        &self,
        path: &Path,
        dsi_names: &HashMap<String, String>,
        dis_names: &HashMap<String, String>,
        stats: &mut ImportStats,
    ) -> Result<(), String> {
        let mut rdr = csv::Reader::from_path(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

        for result in rdr.records() {
            let record = result.map_err(|e| format!("CSV parse error: {}", e))?;

            let dsi_id = record.get(0).unwrap_or("").to_string();
            let dis_id = record.get(2).unwrap_or("").to_string();
            let source = record.get(3).unwrap_or("").to_string();

            let ingredient_name = dsi_names.get(&dsi_id).cloned().unwrap_or(dsi_id.clone());
            let disease_name = dis_names.get(&dis_id).cloned().unwrap_or(dis_id.clone());

            let key = format!("{}_{}", slug(&ingredient_name), slug(&disease_name));

            let _: Option<IdiskEffectivenessRecordWithId> = self
                .db
                .create(("idisk_effectiveness", key.as_str()))
                .content(IdiskEffectivenessRecord {
                    ingredient_id: slug(&ingredient_name),
                    disease_id: slug(&disease_name),
                    disease_name,
                    source,
                })
                .await
                .unwrap_or(None);

            stats.effectiveness += 1;
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Query helpers (used by GraphAction executor)
    // -----------------------------------------------------------------------

    /// Find all adverse reactions for an ingredient.
    pub async fn adverse_reactions_for(
        &self,
        ingredient_name: &str,
    ) -> Vec<(String, String)> {
        let slug_name = slug(ingredient_name);
        let all: Vec<IdiskAdverseReactionRecordWithId> = self
            .db
            .select("idisk_adverse_reaction")
            .await
            .unwrap_or_default();
        all.into_iter()
            .filter(|r| r.ingredient_id == slug_name)
            .map(|r| (r.symptom_id, r.source))
            .collect()
    }

    /// Find all drug interactions for an ingredient.
    pub async fn interactions_for(
        &self,
        ingredient_name: &str,
    ) -> Vec<DrugInteraction> {
        let slug_name = slug(ingredient_name);
        let all: Vec<IdiskInteractionRecordWithId> = self
            .db
            .select("idisk_interaction")
            .await
            .unwrap_or_default();
        all.into_iter()
            .filter(|r| r.ingredient_id == slug_name)
            .map(|r| DrugInteraction {
                drug_id: r.drug_id,
                source: r.source,
                rating: r.interaction_rating,
                description: r.description,
            })
            .collect()
    }

    /// Find interactions between a set of candidate ingredients and a drug name.
    pub async fn interactions_with_drug(
        &self,
        candidate_ingredients: &[String],
        drug_name: &str,
    ) -> Vec<(String, DrugInteraction)> {
        let drug_slug = slug(drug_name);
        let all: Vec<IdiskInteractionRecordWithId> = self
            .db
            .select("idisk_interaction")
            .await
            .unwrap_or_default();

        let candidate_slugs: Vec<String> = candidate_ingredients
            .iter()
            .map(|n| slug(n))
            .collect();

        all.into_iter()
            .filter(|r| {
                candidate_slugs.contains(&r.ingredient_id)
                    && (r.drug_id == drug_slug
                        // Also check if the drug name appears in the description
                        || r.description
                            .as_ref()
                            .map(|d| d.to_lowercase().contains(&drug_name.to_lowercase()))
                            .unwrap_or(false))
            })
            .map(|r| {
                (
                    r.ingredient_id.clone(),
                    DrugInteraction {
                        drug_id: r.drug_id,
                        source: r.source,
                        rating: r.interaction_rating,
                        description: r.description,
                    },
                )
            })
            .collect()
    }

    /// Get mechanism of action text for an ingredient.
    pub async fn mechanism_of_action(&self, ingredient_name: &str) -> Option<String> {
        let slug_name = slug(ingredient_name);
        let rec: Option<IdiskIngredientRecordWithId> = self
            .db
            .select(("idisk_ingredient", slug_name.as_str()))
            .await
            .ok()?;
        rec.and_then(|r| r.mechanism_of_action)
    }

    /// Get safety text for an ingredient.
    pub async fn safety_text(&self, ingredient_name: &str) -> Option<String> {
        let slug_name = slug(ingredient_name);
        let rec: Option<IdiskIngredientRecordWithId> = self
            .db
            .select(("idisk_ingredient", slug_name.as_str()))
            .await
            .ok()?;
        rec.and_then(|r| r.safety)
    }
}

/// A drug interaction record with human-readable fields.
#[derive(Debug, Clone)]
pub struct DrugInteraction {
    pub drug_id: String,
    pub source: String,
    pub rating: Option<String>,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn slug(s: &str) -> String {
    s.to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "_")
        .trim_matches('_')
        .to_string()
}

/// Heuristic archetype assignment based on symptom name keywords.
/// This is the "LLM-assisted assignment" step — manual for now,
/// will be replaced with proper LLM classification for the full 392.
fn guess_archetype(name: &str, aliases: &[String]) -> String {
    let lower = name.to_lowercase();
    let all_text: String = format!(
        "{} {}",
        lower,
        aliases.join(" ").to_lowercase()
    );

    if all_text.contains("pain")
        || all_text.contains("ache")
        || all_text.contains("cramp")
        || all_text.contains("sore")
        || all_text.contains("tender")
    {
        return "pain".into();
    }
    if all_text.contains("sleep")
        || all_text.contains("insomnia")
        || all_text.contains("drowsy")
        || all_text.contains("somnolence")
    {
        return "sleep".into();
    }
    if all_text.contains("depress")
        || all_text.contains("anxiety")
        || all_text.contains("mood")
        || all_text.contains("mania")
        || all_text.contains("irritab")
    {
        return "mood".into();
    }
    if all_text.contains("nausea")
        || all_text.contains("vomit")
        || all_text.contains("diarrhea")
        || all_text.contains("constipat")
        || all_text.contains("bloat")
        || all_text.contains("digest")
        || all_text.contains("gastro")
        || all_text.contains("abdomin")
    {
        return "digestive".into();
    }
    if all_text.contains("fatigue")
        || all_text.contains("tired")
        || all_text.contains("lethargy")
        || all_text.contains("weakness")
        || all_text.contains("malaise")
    {
        return "fatigue".into();
    }
    if all_text.contains("rash")
        || all_text.contains("itch")
        || all_text.contains("skin")
        || all_text.contains("dermati")
        || all_text.contains("acne")
        || all_text.contains("hives")
    {
        return "skin".into();
    }
    if all_text.contains("cough")
        || all_text.contains("wheez")
        || all_text.contains("breath")
        || all_text.contains("asthma")
        || all_text.contains("pulmon")
    {
        return "respiratory".into();
    }
    if all_text.contains("heart")
        || all_text.contains("palpitat")
        || all_text.contains("cardiac")
        || all_text.contains("blood pressure")
        || all_text.contains("hypertens")
    {
        return "cardiovascular".into();
    }
    if all_text.contains("inflam")
        || all_text.contains("swelling")
        || all_text.contains("immune")
        || all_text.contains("allerg")
        || all_text.contains("fever")
    {
        return "immune".into();
    }
    if all_text.contains("memory")
        || all_text.contains("confus")
        || all_text.contains("dizzy")
        || all_text.contains("headache")
        || all_text.contains("numb")
        || all_text.contains("tingling")
        || all_text.contains("seizure")
        || all_text.contains("tremor")
    {
        return "cognitive".into();
    }

    // Default — most unclassifiable symptoms are general
    "fatigue".into()
}
