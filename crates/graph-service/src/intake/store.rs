// ---------------------------------------------------------------------------
// Intake Graph Store — SurrealDB persistence for the intake knowledge graph.
//
// Uses the same SurrealDB instance as the supplement KG but with intake_
// prefixed tables for namespace separation.
// ---------------------------------------------------------------------------

use surrealdb::engine::local::Db;
use surrealdb::Surreal;
use surrealdb_types::{RecordId, SurrealValue};

use super::types::*;

// ---------------------------------------------------------------------------
// DB record types — what SurrealDB stores
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, SurrealValue)]
struct StageRecord {
    stage_id: String,
    description: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct StageRecordWithId {
    id: RecordId,
    stage_id: String,
    description: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct QuestionRecord {
    template: String,
    oldcarts_dimension: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct QuestionRecordWithId {
    id: RecordId,
    template: String,
    oldcarts_dimension: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct GoalRecord {
    description: String,
    fulfilled_by: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct GoalRecordWithId {
    id: RecordId,
    description: String,
    fulfilled_by: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct ArchetypeRecord {
    name: String,
    relevant_oldcarts: String,   // JSON array
    irrelevant_oldcarts: String, // JSON array
    sufficient_dimensions: u32,
    default_systems: String, // JSON array
}

#[derive(Debug, Clone, SurrealValue)]
struct ArchetypeRecordWithId {
    id: RecordId,
    name: String,
    relevant_oldcarts: String,
    irrelevant_oldcarts: String,
    sufficient_dimensions: u32,
    default_systems: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct SymptomProfileRecord {
    name: String,
    cui: Option<String>,
    aliases: String,             // JSON array
    archetype_id: String,
    relevant_override: Option<String>,   // JSON array
    irrelevant_override: Option<String>, // JSON array
    sufficient_override: Option<u32>,
    associated_systems: String, // JSON array
}

#[derive(Debug, Clone, SurrealValue)]
struct SymptomProfileRecordWithId {
    id: RecordId,
    name: String,
    cui: Option<String>,
    aliases: String,
    archetype_id: String,
    relevant_override: Option<String>,
    irrelevant_override: Option<String>,
    sufficient_override: Option<u32>,
    associated_systems: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct ClusterRecord {
    name: String,
    description: String,
    member_symptoms: String, // JSON array
    prioritized_systems: String, // JSON array
}

#[derive(Debug, Clone, SurrealValue)]
struct ClusterRecordWithId {
    id: RecordId,
    name: String,
    description: String,
    member_symptoms: String,
    prioritized_systems: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct SystemReviewRecord {
    system_name: String,
    screening_questions: String, // JSON array
}

#[derive(Debug, Clone, SurrealValue)]
struct SystemReviewRecordWithId {
    id: RecordId,
    system_name: String,
    screening_questions: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct GraphActionRecord {
    action_type: String,
    description: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct GraphActionRecordWithId {
    id: RecordId,
    action_type: String,
    description: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct IntakeEdgeRecord {
    source: String,
    target: String,
    edge_type: String,
    priority: f64,
    required: bool,
    safety_gate: bool,
    max_asks: u32,
    condition: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct IntakeEdgeRecordWithId {
    id: RecordId,
    source: String,
    target: String,
    edge_type: String,
    priority: f64,
    required: bool,
    safety_gate: bool,
    max_asks: u32,
    condition: Option<String>,
}

#[derive(Debug, Clone, SurrealValue)]
struct ExitConditionRecord {
    description: String,
    condition_type: String,
}

#[derive(Debug, Clone, SurrealValue)]
struct ExitConditionRecordWithId {
    id: RecordId,
    description: String,
    condition_type: String,
}

// ---------------------------------------------------------------------------
// IntakeGraphStore
// ---------------------------------------------------------------------------

pub struct IntakeGraphStore {
    db: Surreal<Db>,
}

impl IntakeGraphStore {
    /// Create a store using an existing SurrealDB connection.
    /// The connection should already be pointed at the right namespace/db.
    pub fn new(db: &Surreal<Db>) -> Self {
        Self { db: db.clone() }
    }

    // -- Stages ---------------------------------------------------------------

    pub async fn add_stage(&self, stage: &IntakeStage) {
        let key = stage.id.as_str();
        let _: Option<StageRecordWithId> = self
            .db
            .upsert(("intake_stage", key))
            .content(StageRecord {
                stage_id: key.to_string(),
                description: stage.description.clone(),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_stage(&self, id: &IntakeStageId) -> Option<IntakeStage> {
        let rec: Option<StageRecordWithId> =
            self.db.select(("intake_stage", id.as_str())).await.ok()?;
        rec.map(|r| IntakeStage {
            id: id.clone(),
            description: r.description,
        })
    }

    // -- Question Templates ---------------------------------------------------

    pub async fn add_question(&self, q: &QuestionTemplate) {
        let _: Option<QuestionRecordWithId> = self
            .db
            .upsert(("intake_question", q.id.as_str()))
            .content(QuestionRecord {
                template: q.template.clone(),
                oldcarts_dimension: q.oldcarts_dimension.map(|d| d.as_str().to_string()),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_question(&self, id: &str) -> Option<QuestionTemplate> {
        let rec: Option<QuestionRecordWithId> =
            self.db.select(("intake_question", id)).await.ok()?;
        rec.map(|r| QuestionTemplate {
            id: id.to_string(),
            template: r.template,
            oldcarts_dimension: r
                .oldcarts_dimension
                .and_then(|s| parse_oldcarts_dimension(&s)),
        })
    }

    pub async fn all_questions(&self) -> Vec<QuestionTemplate> {
        let recs: Vec<QuestionRecordWithId> = self
            .db
            .select("intake_question")
            .await
            .unwrap_or_default();
        recs.into_iter()
            .map(|r| QuestionTemplate {
                id: record_key(&r.id),
                template: r.template,
                oldcarts_dimension: r
                    .oldcarts_dimension
                    .and_then(|s| parse_oldcarts_dimension(&s)),
            })
            .collect()
    }

    // -- Clinical Goals -------------------------------------------------------

    pub async fn add_goal(&self, g: &ClinicalGoal) {
        let _: Option<GoalRecordWithId> = self
            .db
            .upsert(("intake_goal", g.id.as_str()))
            .content(GoalRecord {
                description: g.description.clone(),
                fulfilled_by: g.fulfilled_by.as_ref().map(|f| format!("{:?}", f)),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_goal(&self, id: &str) -> Option<ClinicalGoal> {
        let rec: Option<GoalRecordWithId> = self.db.select(("intake_goal", id)).await.ok()?;
        rec.map(|r| ClinicalGoal {
            id: id.to_string(),
            description: r.description,
            fulfilled_by: r.fulfilled_by.and_then(|s| parse_extractor_field(&s)),
        })
    }

    // -- Archetype Profiles ---------------------------------------------------

    pub async fn add_archetype(&self, a: &ArchetypeProfile) {
        let _: Option<ArchetypeRecordWithId> = self
            .db
            .upsert(("intake_archetype", a.id.as_str()))
            .content(ArchetypeRecord {
                name: a.name.clone(),
                relevant_oldcarts: serde_json::to_string(&a.relevant_oldcarts)
                    .unwrap_or_default(),
                irrelevant_oldcarts: serde_json::to_string(&a.irrelevant_oldcarts)
                    .unwrap_or_default(),
                sufficient_dimensions: a.sufficient_dimensions as u32,
                default_systems: serde_json::to_string(&a.default_systems).unwrap_or_default(),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_archetype(&self, id: &str) -> Option<ArchetypeProfile> {
        let rec: Option<ArchetypeRecordWithId> =
            self.db.select(("intake_archetype", id)).await.ok()?;
        rec.map(|r| ArchetypeProfile {
            id: id.to_string(),
            name: r.name,
            relevant_oldcarts: serde_json::from_str(&r.relevant_oldcarts).unwrap_or_default(),
            irrelevant_oldcarts: serde_json::from_str(&r.irrelevant_oldcarts).unwrap_or_default(),
            sufficient_dimensions: r.sufficient_dimensions as u8,
            default_systems: serde_json::from_str(&r.default_systems).unwrap_or_default(),
        })
    }

    pub async fn all_archetypes(&self) -> Vec<ArchetypeProfile> {
        let recs: Vec<ArchetypeRecordWithId> = self
            .db
            .select("intake_archetype")
            .await
            .unwrap_or_default();
        recs.into_iter()
            .map(|r| ArchetypeProfile {
                id: record_key(&r.id),
                name: r.name,
                relevant_oldcarts: serde_json::from_str(&r.relevant_oldcarts).unwrap_or_default(),
                irrelevant_oldcarts: serde_json::from_str(&r.irrelevant_oldcarts)
                    .unwrap_or_default(),
                sufficient_dimensions: r.sufficient_dimensions as u8,
                default_systems: serde_json::from_str(&r.default_systems).unwrap_or_default(),
            })
            .collect()
    }

    // -- Symptom Profiles -----------------------------------------------------

    pub async fn add_symptom_profile(&self, sp: &SymptomProfile) {
        let _: Option<SymptomProfileRecordWithId> = self
            .db
            .upsert(("intake_symptom_profile", sp.id.as_str()))
            .content(SymptomProfileRecord {
                name: sp.name.clone(),
                cui: sp.cui.clone(),
                aliases: serde_json::to_string(&sp.aliases).unwrap_or_default(),
                archetype_id: sp.archetype_id.clone(),
                relevant_override: sp
                    .relevant_oldcarts_override
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_default()),
                irrelevant_override: sp
                    .irrelevant_oldcarts_override
                    .as_ref()
                    .map(|v| serde_json::to_string(v).unwrap_or_default()),
                sufficient_override: sp.sufficient_dimensions_override.map(|v| v as u32),
                associated_systems: serde_json::to_string(&sp.associated_systems)
                    .unwrap_or_default(),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_symptom_profile(&self, id: &str) -> Option<SymptomProfile> {
        let rec: Option<SymptomProfileRecordWithId> = self
            .db
            .select(("intake_symptom_profile", id))
            .await
            .ok()?;
        rec.map(|r| SymptomProfile {
            id: id.to_string(),
            name: r.name,
            cui: r.cui,
            aliases: serde_json::from_str(&r.aliases).unwrap_or_default(),
            archetype_id: r.archetype_id,
            relevant_oldcarts_override: r
                .relevant_override
                .and_then(|s| serde_json::from_str(&s).ok()),
            irrelevant_oldcarts_override: r
                .irrelevant_override
                .and_then(|s| serde_json::from_str(&s).ok()),
            sufficient_dimensions_override: r.sufficient_override.map(|v| v as u8),
            associated_systems: serde_json::from_str(&r.associated_systems).unwrap_or_default(),
        })
    }

    /// Find a symptom profile by name or alias (case-insensitive).
    ///
    /// Matching tiers (first match wins):
    ///   1. Exact ID match (slug of input)
    ///   2. Exact name or alias match
    ///   3. Input contains name, or name/alias contains input (substring)
    pub async fn find_symptom_profile(&self, name: &str) -> Option<SymptomProfile> {
        let lower = name.to_lowercase().trim().to_string();

        // Tier 1: exact ID match
        if let Some(sp) = self.get_symptom_profile(&slug(&lower)).await {
            return Some(sp);
        }

        let all: Vec<SymptomProfileRecordWithId> = self
            .db
            .select("intake_symptom_profile")
            .await
            .unwrap_or_default();

        // Tier 2: exact name or alias match
        for r in &all {
            if r.name.to_lowercase() == lower {
                return self.get_symptom_profile(&record_key(&r.id)).await;
            }
            let aliases: Vec<String> = serde_json::from_str(&r.aliases).unwrap_or_default();
            for alias in &aliases {
                if alias.to_lowercase() == lower {
                    return self.get_symptom_profile(&record_key(&r.id)).await;
                }
            }
        }

        // Tier 3: substring match — input contains profile name, or profile name/alias
        // contains input. Prefers the longest alias match to avoid over-broad matches.
        let mut best: Option<(String, usize)> = None; // (record_key, match_len)
        for r in &all {
            let profile_name_lower = r.name.to_lowercase();
            let aliases: Vec<String> = serde_json::from_str(&r.aliases).unwrap_or_default();

            // Check if input contains the profile name (e.g. "tension headaches" contains "headache")
            if lower.contains(&profile_name_lower) || profile_name_lower.contains(&lower) {
                let match_len = profile_name_lower.len();
                if best.as_ref().map_or(true, |(_, l)| match_len > *l) {
                    best = Some((record_key(&r.id), match_len));
                }
            }
            for alias in &aliases {
                let alias_lower = alias.to_lowercase();
                if lower.contains(&alias_lower) || alias_lower.contains(&lower) {
                    let match_len = alias_lower.len();
                    if best.as_ref().map_or(true, |(_, l)| match_len > *l) {
                        best = Some((record_key(&r.id), match_len));
                    }
                }
            }
        }
        if let Some((key, _)) = best {
            return self.get_symptom_profile(&key).await;
        }

        None
    }

    pub async fn symptom_profile_count(&self) -> usize {
        let all: Vec<SymptomProfileRecordWithId> = self
            .db
            .select("intake_symptom_profile")
            .await
            .unwrap_or_default();
        all.len()
    }

    /// Return all symptom profile IDs — used by the symptom resolver to
    /// build the closed-vocabulary list for the LLM classifier.
    pub async fn all_symptom_profile_ids(&self) -> Vec<String> {
        let all: Vec<SymptomProfileRecordWithId> = self
            .db
            .select("intake_symptom_profile")
            .await
            .unwrap_or_default();
        all.iter().map(|r| record_key(&r.id)).collect()
    }

    // -- Symptom Clusters -----------------------------------------------------

    pub async fn add_cluster(&self, c: &SymptomCluster) {
        let _: Option<ClusterRecordWithId> = self
            .db
            .upsert(("intake_cluster", c.id.as_str()))
            .content(ClusterRecord {
                name: c.name.clone(),
                description: c.description.clone(),
                member_symptoms: serde_json::to_string(&c.member_symptoms).unwrap_or_default(),
                prioritized_systems: serde_json::to_string(&c.prioritized_systems)
                    .unwrap_or_default(),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_cluster(&self, id: &str) -> Option<SymptomCluster> {
        let rec: Option<ClusterRecordWithId> =
            self.db.select(("intake_cluster", id)).await.ok()?;
        rec.map(|r| SymptomCluster {
            id: id.to_string(),
            name: r.name,
            description: r.description,
            member_symptoms: serde_json::from_str(&r.member_symptoms).unwrap_or_default(),
            prioritized_systems: serde_json::from_str(&r.prioritized_systems).unwrap_or_default(),
        })
    }

    /// Find all clusters that contain a given symptom profile ID.
    pub async fn clusters_for_symptom(&self, symptom_id: &str) -> Vec<SymptomCluster> {
        let all: Vec<ClusterRecordWithId> = self
            .db
            .select("intake_cluster")
            .await
            .unwrap_or_default();
        let mut result = Vec::new();
        for r in all {
            let members: Vec<String> =
                serde_json::from_str(&r.member_symptoms).unwrap_or_default();
            if members.iter().any(|m| m == symptom_id) {
                result.push(SymptomCluster {
                    id: record_key(&r.id),
                    name: r.name,
                    description: r.description,
                    member_symptoms: members,
                    prioritized_systems: serde_json::from_str(&r.prioritized_systems)
                        .unwrap_or_default(),
                });
            }
        }
        result
    }

    // -- System Reviews -------------------------------------------------------

    pub async fn add_system_review(&self, sr: &SystemReviewNode) {
        let _: Option<SystemReviewRecordWithId> = self
            .db
            .upsert(("intake_system_review", sr.id.as_str()))
            .content(SystemReviewRecord {
                system_name: sr.system_name.clone(),
                screening_questions: serde_json::to_string(&sr.screening_questions)
                    .unwrap_or_default(),
            })
            .await
            .unwrap_or(None);
    }

    pub async fn get_system_review(&self, id: &str) -> Option<SystemReviewNode> {
        let rec: Option<SystemReviewRecordWithId> = self
            .db
            .select(("intake_system_review", id))
            .await
            .ok()?;
        rec.map(|r| SystemReviewNode {
            id: id.to_string(),
            system_name: r.system_name,
            screening_questions: serde_json::from_str(&r.screening_questions).unwrap_or_default(),
        })
    }

    pub async fn find_system_review(&self, system_name: &str) -> Option<SystemReviewNode> {
        let lower = system_name.to_lowercase();
        let all: Vec<SystemReviewRecordWithId> = self
            .db
            .select("intake_system_review")
            .await
            .unwrap_or_default();
        for r in all {
            if r.system_name.to_lowercase() == lower {
                return Some(SystemReviewNode {
                    id: record_key(&r.id),
                    system_name: r.system_name,
                    screening_questions: serde_json::from_str(&r.screening_questions)
                        .unwrap_or_default(),
                });
            }
        }
        None
    }

    // -- Graph Actions --------------------------------------------------------

    pub async fn add_graph_action(&self, ga: &GraphActionNode) {
        let _: Option<GraphActionRecordWithId> = self
            .db
            .upsert(("intake_graph_action", ga.id.as_str()))
            .content(GraphActionRecord {
                action_type: format!("{:?}", ga.action_type),
                description: ga.description.clone(),
            })
            .await
            .unwrap_or(None);
    }

    // -- Exit Conditions ------------------------------------------------------

    pub async fn add_exit_condition(&self, ec: &ExitCondition) {
        let _: Option<ExitConditionRecordWithId> = self
            .db
            .upsert(("intake_exit_condition", ec.id.as_str()))
            .content(ExitConditionRecord {
                description: ec.description.clone(),
                condition_type: format!("{:?}", ec.condition),
            })
            .await
            .unwrap_or(None);
    }

    // -- Edges ----------------------------------------------------------------

    pub async fn add_edge(
        &self,
        source: &str,
        target: &str,
        edge_type: IntakeEdgeType,
        meta: IntakeEdgeMeta,
    ) {
        let _: Option<IntakeEdgeRecordWithId> = self
            .db
            .create("intake_edge")
            .content(IntakeEdgeRecord {
                source: source.to_string(),
                target: target.to_string(),
                edge_type: format!("{:?}", edge_type),
                priority: meta.priority,
                required: meta.required,
                safety_gate: meta.safety_gate,
                max_asks: meta.max_asks as u32,
                condition: meta.condition,
            })
            .await
            .unwrap_or(None);
    }

    /// Get all edges from a given source node ID.
    pub async fn edges_from(&self, source_id: &str) -> Vec<(String, IntakeEdgeType, IntakeEdgeMeta)> {
        let all: Vec<IntakeEdgeRecordWithId> = self
            .db
            .select("intake_edge")
            .await
            .unwrap_or_default();
        all.into_iter()
            .filter(|e| e.source == source_id)
            .filter_map(|e| {
                let edge_type = parse_intake_edge_type(&e.edge_type)?;
                Some((
                    e.target,
                    edge_type,
                    IntakeEdgeMeta {
                        priority: e.priority,
                        required: e.required,
                        safety_gate: e.safety_gate,
                        max_asks: e.max_asks as u8,
                        condition: e.condition,
                    },
                ))
            })
            .collect()
    }

    /// Get all edges pointing to a given target node ID.
    pub async fn edges_to(&self, target_id: &str) -> Vec<(String, IntakeEdgeType, IntakeEdgeMeta)> {
        let all: Vec<IntakeEdgeRecordWithId> = self
            .db
            .select("intake_edge")
            .await
            .unwrap_or_default();
        all.into_iter()
            .filter(|e| e.target == target_id)
            .filter_map(|e| {
                let edge_type = parse_intake_edge_type(&e.edge_type)?;
                Some((
                    e.source,
                    edge_type,
                    IntakeEdgeMeta {
                        priority: e.priority,
                        required: e.required,
                        safety_gate: e.safety_gate,
                        max_asks: e.max_asks as u8,
                        condition: e.condition,
                    },
                ))
            })
            .collect()
    }

    // -- Stats ----------------------------------------------------------------

    pub async fn edge_count(&self) -> usize {
        let all: Vec<IntakeEdgeRecordWithId> =
            self.db.select("intake_edge").await.unwrap_or_default();
        all.len()
    }
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

fn record_key(id: &RecordId) -> String {
    match &id.key {
        surrealdb_types::RecordIdKey::String(s) => s.clone(),
        other => format!("{:?}", other),
    }
}

fn parse_oldcarts_dimension(s: &str) -> Option<OldcartsDimension> {
    match s.to_lowercase().as_str() {
        "onset" => Some(OldcartsDimension::Onset),
        "location" => Some(OldcartsDimension::Location),
        "duration" => Some(OldcartsDimension::Duration),
        "character" => Some(OldcartsDimension::Character),
        "aggravating" => Some(OldcartsDimension::Aggravating),
        "alleviating" => Some(OldcartsDimension::Alleviating),
        "radiation" => Some(OldcartsDimension::Radiation),
        "timing" => Some(OldcartsDimension::Timing),
        "severity" => Some(OldcartsDimension::Severity),
        _ => None,
    }
}

fn parse_extractor_field(s: &str) -> Option<ExtractorField> {
    match s {
        "Symptom" => Some(ExtractorField::Symptom),
        "System" => Some(ExtractorField::System),
        "Onset" => Some(ExtractorField::Onset),
        "Location" => Some(ExtractorField::Location),
        "Duration" => Some(ExtractorField::Duration),
        "Character" => Some(ExtractorField::Character),
        "Aggravating" => Some(ExtractorField::Aggravating),
        "Alleviating" => Some(ExtractorField::Alleviating),
        "Radiation" => Some(ExtractorField::Radiation),
        "Timing" => Some(ExtractorField::Timing),
        "Severity" => Some(ExtractorField::Severity),
        "DeniedSystem" => Some(ExtractorField::DeniedSystem),
        "Medication" => Some(ExtractorField::Medication),
        "Engagement" => Some(ExtractorField::Engagement),
        _ => None,
    }
}

fn parse_intake_edge_type(s: &str) -> Option<IntakeEdgeType> {
    match s {
        "HasStage" => Some(IntakeEdgeType::HasStage),
        "Asks" => Some(IntakeEdgeType::Asks),
        "Fulfills" => Some(IntakeEdgeType::Fulfills),
        "FallsBack" => Some(IntakeEdgeType::FallsBack),
        "RelevantFor" => Some(IntakeEdgeType::RelevantFor),
        "IrrelevantFor" => Some(IntakeEdgeType::IrrelevantFor),
        "BelongsTo" => Some(IntakeEdgeType::BelongsTo),
        "CoOccurs" => Some(IntakeEdgeType::CoOccurs),
        "Suggests" => Some(IntakeEdgeType::Suggests),
        "Triggers" => Some(IntakeEdgeType::Triggers),
        "ExitsWhen" => Some(IntakeEdgeType::ExitsWhen),
        "EscalatesTo" => Some(IntakeEdgeType::EscalatesTo),
        "Probes" => Some(IntakeEdgeType::Probes),
        "HasGoal" => Some(IntakeEdgeType::HasGoal),
        _ => None,
    }
}
