use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::intake::idisk::DrugInteraction;
use crate::source::{
    CitationRecord, EdgeAgreement, EdgeQuality, EdgeWithQuality, MultiProviderEdge,
    ProviderObservation,
};
use crate::graph::NodeIndex;
use crate::types::{EdgeData, EdgeMetadata, EdgeType, NodeData, NodeType, Source};

// ---------------------------------------------------------------------------
// SupplementClient — HTTP client for supplementology API /rpc/ endpoints.
//
// Used by the web-server and query engine for all read queries. The NSAI loop
// uses KnowledgeGraph directly (PgPool) for writes.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SupplementClient {
    client: Client,
    base_url: String,
}

// -- Wire types from the API ------------------------------------------------

#[derive(Deserialize)]
struct FindNodeResponse {
    entity_id: Option<Uuid>,
    entity_name: Option<String>,
    entity_type: Option<String>,
}

#[derive(Deserialize)]
struct EdgeRow {
    neighbor_id: Uuid,
    neighbor_name: String,
    neighbor_type: String,
    rel_type: String,
    confidence: f32,
    complexity: f32,
    source: String,
    attrs: Value,
}

#[derive(Deserialize)]
struct EdgesResponse {
    edges: Vec<EdgeRow>,
}

#[derive(Deserialize)]
struct NodeRow {
    entity_id: Uuid,
    entity_name: String,
    #[allow(dead_code)]
    entity_type: String,
}

#[derive(Deserialize)]
struct NodesResponse {
    nodes: Vec<NodeRow>,
}

#[derive(Deserialize)]
struct CitationRow {
    pmid: Option<String>,
    sentence: String,
    confidence: f32,
    edge_type: Option<String>,
    suppkg_predicate: Option<String>,
    target_node: Option<String>,
    target_cui: Option<String>,
    source_cui: Option<String>,
}

#[derive(Deserialize)]
struct CitationsResponse {
    citations: Vec<CitationRow>,
}

#[derive(Deserialize)]
struct MultiProviderRow {
    source_node: String,
    target_node: String,
    edge_type: String,
    providers: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct MultiProviderResponse {
    edges: Vec<MultiProviderRow>,
}

#[derive(Deserialize)]
struct ObservationRow {
    provider: String,
    model: String,
    observation_type: String,
    confidence: f32,
}

#[derive(Deserialize)]
struct ProviderAgreementResponse {
    provider_count: usize,
    total_observations: usize,
    observations: Vec<ObservationRow>,
}

#[derive(Deserialize)]
struct QualityRow {
    source_node: String,
    target_node: String,
    edge_type: String,
    provider_count: Option<i64>,
    total_observations: Option<i64>,
    has_citation: Option<bool>,
    quality_tier: Option<String>,
}

#[derive(Deserialize)]
struct QualityResponse {
    edges: Vec<QualityRow>,
}

#[derive(Deserialize)]
struct IngredientRow {
    name: String,
}

#[derive(Deserialize)]
struct IngredientsResponse {
    ingredients: Vec<IngredientRow>,
}

#[derive(Deserialize)]
struct NarrativesResponse {
    mechanism_of_action: Option<String>,
}

#[derive(Deserialize)]
struct AdverseReactionRow {
    symptom_slug: String,
    source: String,
}

#[derive(Deserialize)]
struct AdverseReactionsResponse {
    adverse_reactions: Vec<AdverseReactionRow>,
}

#[derive(Deserialize)]
struct DrugInteractionRow {
    drug: String,
    source: String,
    description: Option<String>,
    rating: Option<String>,
}

#[derive(Deserialize)]
struct DrugInteractionsResponse {
    interactions: Vec<DrugInteractionRow>,
}

// ---------------------------------------------------------------------------

impl SupplementClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("failed to build reqwest client"),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Option<T> {
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<T>().await.ok()
    }

    // -- Node lookup ---------------------------------------------------------

    pub async fn find_node(&self, name: &str) -> Option<(NodeIndex, NodeData)> {
        let resp: Value = self.client
            .get(self.url("/rpc/find_node"))
            .query(&[("name", name)])
            .send().await.ok()?
            .json().await.ok()?;

        if resp.is_null() {
            return None;
        }

        let row: FindNodeResponse = serde_json::from_value(resp).ok()?;
        let id = row.entity_id?;
        let name = row.entity_name?;
        let type_str = row.entity_type?;
        let node_type = api_type_to_node_type(&type_str)?;
        Some((NodeIndex(id), NodeData::new(name, node_type)))
    }

    pub async fn nodes_by_type(&self, node_type: &NodeType) -> Vec<NodeIndex> {
        let type_name = node_type_to_api_str(node_type);
        let resp: NodesResponse = match self.get_json(
            self.client.get(self.url("/rpc/nodes_by_type"))
                .query(&[("type_name", type_name)])
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.nodes.into_iter().map(|r| NodeIndex(r.entity_id)).collect()
    }

    pub async fn known_ingredients(&self) -> Vec<String> {
        let resp: IngredientsResponse = match self.get_json(
            self.client.get(self.url("/v1/graph/ingredients"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.ingredients.into_iter().map(|r| r.name).collect()
    }

    // -- Edge traversal ------------------------------------------------------

    pub async fn outgoing_edges(
        &self,
        idx: &NodeIndex,
        min_confidence: f32,
    ) -> Vec<(NodeIndex, EdgeData)> {
        let resp: EdgesResponse = match self.get_json(
            self.client.get(self.url("/rpc/outgoing_edges"))
                .query(&[
                    ("entity_id", idx.0.to_string()),
                    ("min_confidence", min_confidence.to_string()),
                ])
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.edges.into_iter().filter_map(edge_row_to_data).collect()
    }

    pub async fn incoming_edges(
        &self,
        idx: &NodeIndex,
        min_confidence: f32,
    ) -> Vec<(NodeIndex, EdgeData)> {
        let resp: EdgesResponse = match self.get_json(
            self.client.get(self.url("/rpc/incoming_edges"))
                .query(&[
                    ("entity_id", idx.0.to_string()),
                    ("min_confidence", min_confidence.to_string()),
                ])
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.edges.into_iter().filter_map(edge_row_to_data).collect()
    }

    // -- Source / quality ----------------------------------------------------

    pub async fn edges_by_quality(&self) -> Vec<EdgeWithQuality> {
        let resp: QualityResponse = match self.get_json(
            self.client.get(self.url("/v1/graph/edge-quality"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };

        resp.edges.into_iter().map(|r| {
            let provider_count = r.provider_count.unwrap_or(0) as usize;
            let total_observations = r.total_observations.unwrap_or(0) as usize;
            let has_citation = r.has_citation.unwrap_or(false);
            let quality = match r.quality_tier.as_deref() {
                Some("citation_backed") => EdgeQuality::CitationBacked,
                Some("multi_provider")  => EdgeQuality::MultiProvider,
                Some("single_provider") => EdgeQuality::SingleProvider,
                Some("speculative")     => EdgeQuality::Speculative,
                _                       => EdgeQuality::Deduced,
            };
            EdgeWithQuality {
                source_node: r.source_node,
                target_node: r.target_node,
                edge_type: r.edge_type,
                quality,
                provider_count,
                total_observations,
                has_citation,
            }
        }).collect()
    }

    pub async fn multi_provider_edges(&self) -> Vec<MultiProviderEdge> {
        let resp: MultiProviderResponse = match self.get_json(
            self.client.get(self.url("/rpc/multi_provider_edges"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.edges.into_iter().map(|r| MultiProviderEdge {
            source_node: r.source_node,
            target_node: r.target_node,
            edge_type: r.edge_type,
            providers: r.providers.unwrap_or_default(),
        }).collect()
    }

    pub async fn provider_agreement(
        &self,
        source_node: &str,
        target_node: &str,
        edge_type: &str,
    ) -> EdgeAgreement {
        let empty = EdgeAgreement { provider_count: 0, total_observations: 0, providers: vec![] };
        let resp: ProviderAgreementResponse = match self.get_json(
            self.client.get(self.url("/rpc/provider_agreement"))
                .query(&[
                    ("source_node", source_node),
                    ("target_node", target_node),
                    ("edge_type", edge_type),
                ])
        ).await {
            Some(r) => r,
            None => return empty,
        };
        EdgeAgreement {
            provider_count: resp.provider_count,
            total_observations: resp.total_observations,
            providers: resp.observations.into_iter().map(|o| ProviderObservation {
                provider: o.provider,
                model: o.model,
                observation_type: o.observation_type,
                confidence: o.confidence as f64,
            }).collect(),
        }
    }

    // -- iDISK safety data ---------------------------------------------------

    pub async fn mechanism_of_action(&self, ingredient: &str) -> Option<String> {
        let resp: NarrativesResponse = self.get_json(
            self.client.get(self.url("/rpc/ingredient_narratives"))
                .query(&[("name", ingredient)])
        ).await?;
        resp.mechanism_of_action
    }

    pub async fn adverse_reactions_for(&self, ingredient: &str) -> Vec<(String, String)> {
        let slug = ingredient.to_lowercase().replace(' ', "_").replace('-', "_");
        let resp: AdverseReactionsResponse = match self.get_json(
            self.client.get(self.url(&format!("/v1/ingredients/{slug}/adverse-reactions")))
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.adverse_reactions.into_iter()
            .map(|r| (r.symptom_slug, r.source))
            .collect()
    }

    pub async fn interactions_with_drug(
        &self,
        candidate_ingredients: &[String],
        drug_name: &str,
    ) -> Vec<(String, DrugInteraction)> {
        let mut results = vec![];
        for ingredient in candidate_ingredients {
            let resp: DrugInteractionsResponse = match self.get_json(
                self.client.get(self.url("/rpc/drug_interactions"))
                    .query(&[("ingredient", ingredient.as_str()), ("drug", drug_name)])
            ).await {
                Some(r) => r,
                None => continue,
            };
            for row in resp.interactions {
                results.push((ingredient.clone(), DrugInteraction {
                    drug_id: row.drug,
                    source: row.source,
                    rating: row.rating,
                    description: row.description,
                }));
            }
        }
        results
    }

    // -- Intake config -------------------------------------------------------

    pub async fn intake_archetypes(&self) -> Vec<crate::intake::types::ArchetypeProfile> {
        #[derive(Deserialize)]
        struct Row {
            id: String,
            name: String,
            sufficient_dimensions: i16,
            relevant_oldcarts: Vec<String>,
            irrelevant_oldcarts: Vec<String>,
            default_systems: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Resp { archetypes: Vec<Row> }

        let resp: Resp = match self.get_json(
            self.client.get(self.url("/v1/intake/archetypes"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };

        use crate::intake::types::OldcartsDimension;
        resp.archetypes.into_iter().map(|r| crate::intake::types::ArchetypeProfile {
            id: r.id,
            name: r.name,
            sufficient_dimensions: r.sufficient_dimensions as u8,
            relevant_oldcarts: r.relevant_oldcarts.iter()
                .filter_map(|s| parse_oldcarts(s)).collect(),
            irrelevant_oldcarts: r.irrelevant_oldcarts.iter()
                .filter_map(|s| parse_oldcarts(s)).collect(),
            default_systems: r.default_systems,
        }).collect()
    }

    pub async fn intake_symptom_profiles(&self) -> Vec<crate::intake::types::SymptomProfile> {
        #[derive(Deserialize)]
        struct Row {
            id: String,
            name: String,
            cui: Option<String>,
            aliases: Vec<String>,
            archetype_id: String,
            sufficient_dimensions_override: Option<i16>,
            relevant_oldcarts_override: Option<Vec<String>>,
            irrelevant_oldcarts_override: Option<Vec<String>>,
            associated_systems: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Resp { profiles: Vec<Row> }

        let resp: Resp = match self.get_json(
            self.client.get(self.url("/v1/intake/symptom-profiles"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };

        resp.profiles.into_iter().map(|r| crate::intake::types::SymptomProfile {
            id: r.id,
            name: r.name,
            cui: r.cui,
            aliases: r.aliases,
            archetype_id: r.archetype_id,
            sufficient_dimensions_override: r.sufficient_dimensions_override.map(|v| v as u8),
            relevant_oldcarts_override: r.relevant_oldcarts_override.map(|v|
                v.iter().filter_map(|s| parse_oldcarts(s)).collect()
            ),
            irrelevant_oldcarts_override: r.irrelevant_oldcarts_override.map(|v|
                v.iter().filter_map(|s| parse_oldcarts(s)).collect()
            ),
            associated_systems: r.associated_systems,
        }).collect()
    }

    pub async fn intake_questions(&self) -> Vec<crate::intake::types::QuestionTemplate> {
        #[derive(Deserialize)]
        struct Row {
            id: String,
            template: String,
            oldcarts_dimension: Option<String>,
            system_name: Option<String>,
        }
        #[derive(Deserialize)]
        struct Resp { questions: Vec<Row> }

        let resp: Resp = match self.get_json(
            self.client.get(self.url("/v1/intake/questions"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };

        resp.questions.into_iter().map(|r| crate::intake::types::QuestionTemplate {
            id: r.id,
            template: r.template,
            oldcarts_dimension: r.oldcarts_dimension.as_deref().and_then(parse_oldcarts),
            system_name: r.system_name,
        }).collect()
    }

    pub async fn intake_clusters(&self) -> Vec<crate::intake::types::SymptomCluster> {
        #[derive(Deserialize)]
        struct Row {
            id: String,
            name: String,
            description: String,
            member_symptoms: Vec<String>,
            prioritized_systems: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Resp { clusters: Vec<Row> }

        let resp: Resp = match self.get_json(
            self.client.get(self.url("/v1/intake/clusters"))
        ).await {
            Some(r) => r,
            None => return vec![],
        };

        resp.clusters.into_iter().map(|r| crate::intake::types::SymptomCluster {
            id: r.id,
            name: r.name,
            description: r.description,
            member_symptoms: r.member_symptoms,
            prioritized_systems: r.prioritized_systems,
        }).collect()
    }

    // -- Citations -----------------------------------------------------------

    pub async fn citations_for_ingredient(&self, ingredient: &str) -> Vec<CitationRecord> {
        let resp: CitationsResponse = match self.get_json(
            self.client.get(self.url("/rpc/citations_for_ingredient"))
                .query(&[("ingredient", ingredient)])
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.citations.into_iter().map(|r| CitationRecord {
            source_node: ingredient.to_string(),
            target_node: r.target_node.unwrap_or_default(),
            edge_type: r.edge_type.unwrap_or_default(),
            pmid: r.pmid.unwrap_or_default(),
            sentence: r.sentence,
            confidence: r.confidence as f64,
            suppkg_predicate: r.suppkg_predicate.unwrap_or_default(),
            source_cui: r.source_cui.unwrap_or_default(),
            target_cui: r.target_cui.unwrap_or_default(),
        }).collect()
    }

    pub async fn citations_for_edge(
        &self,
        source_node: &str,
        target_node: &str,
    ) -> Vec<CitationRecord> {
        let resp: CitationsResponse = match self.get_json(
            self.client.get(self.url("/rpc/citations_for_edge"))
                .query(&[
                    ("source_node", source_node),
                    ("target_node", target_node),
                ])
        ).await {
            Some(r) => r,
            None => return vec![],
        };
        resp.citations.into_iter().map(|r| CitationRecord {
            source_node: source_node.to_string(),
            target_node: target_node.to_string(),
            edge_type: r.edge_type.unwrap_or_default(),
            pmid: r.pmid.unwrap_or_default(),
            sentence: r.sentence,
            confidence: r.confidence as f64,
            suppkg_predicate: r.suppkg_predicate.unwrap_or_default(),
            source_cui: r.source_cui.unwrap_or_default(),
            target_cui: r.target_cui.unwrap_or_default(),
        }).collect()
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn edge_row_to_data(r: EdgeRow) -> Option<(NodeIndex, EdgeData)> {
    let edge_type = api_str_to_edge_type(&r.rel_type)?;
    let node_type = api_type_to_node_type(&r.neighbor_type)?;
    let _ = node_type; // neighbor type used for filtering if needed later
    let metadata = EdgeMetadata {
        confidence: r.confidence as f64,
        source: api_source_to_source(&r.source),
        iteration: r.attrs.get("iteration").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
        epoch: r.attrs.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        llm_agreement: None,
        reasoning_depth: r.attrs.get("reasoning_depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        extra: std::collections::HashMap::new(),
    };
    Some((NodeIndex(r.neighbor_id), EdgeData::new(edge_type, metadata)))
}

fn api_type_to_node_type(s: &str) -> Option<NodeType> {
    match s {
        "organism" | "compound" | "ingredient" => Some(NodeType::Ingredient),
        "system" => Some(NodeType::System),
        "mechanism" => Some(NodeType::Mechanism),
        "symptom" | "biological_symptom" => Some(NodeType::Symptom),
        "property" => Some(NodeType::Property),
        "condition" => Some(NodeType::Condition),
        "substrate" => Some(NodeType::Substrate),
        "pathway" => Some(NodeType::Pathway),
        "biological_process" => Some(NodeType::BiologicalProcess),
        "metabolite" => Some(NodeType::Metabolite),
        "gene_protein" => Some(NodeType::GeneProtein),
        "cell_type" => Some(NodeType::CellType),
        "microbiota" => Some(NodeType::Microbiota),
        "receptor" => Some(NodeType::Receptor),
        _ => None,
    }
}

fn node_type_to_api_str(nt: &NodeType) -> &'static str {
    match nt {
        NodeType::Ingredient => "ingredient",
        NodeType::System => "system",
        NodeType::Mechanism => "mechanism",
        NodeType::Symptom => "symptom",
        NodeType::Property => "property",
        NodeType::Condition => "condition",
        NodeType::Substrate => "substrate",
        NodeType::Pathway => "pathway",
        NodeType::BiologicalProcess => "biological_process",
        NodeType::Metabolite => "metabolite",
        NodeType::GeneProtein => "gene_protein",
        NodeType::CellType => "cell_type",
        NodeType::Microbiota => "microbiota",
        NodeType::Receptor => "receptor",
    }
}

fn api_str_to_edge_type(s: &str) -> Option<EdgeType> {
    match s {
        "acts_on" => Some(EdgeType::ActsOn),
        "via_mechanism" => Some(EdgeType::ViaMechanism),
        "affords" => Some(EdgeType::Affords),
        "presents_in" => Some(EdgeType::PresentsIn),
        "modulates" => Some(EdgeType::Modulates),
        "contraindicated_with" => Some(EdgeType::ContraindicatedWith),
        "competes_with" => Some(EdgeType::CompetesWith),
        "disinhibits" => Some(EdgeType::Disinhibits),
        "sequesters" => Some(EdgeType::Sequesters),
        "releases" => Some(EdgeType::Releases),
        "amplifies" => Some(EdgeType::Amplifies),
        "desensitizes" => Some(EdgeType::Desensitizes),
        "positively_reinforces" => Some(EdgeType::PositivelyReinforces),
        "gates" => Some(EdgeType::Gates),
        _ => None,
    }
}

fn api_source_to_source(s: &str) -> Source {
    match s {
        "nsai_emergent" => Source::StructurallyEmergent,
        "nsai_deduced" => Source::Deduced,
        _ => Source::Extracted,
    }
}

fn parse_oldcarts(s: &str) -> Option<crate::intake::types::OldcartsDimension> {
    use crate::intake::types::OldcartsDimension::*;
    match s {
        "onset"       => Some(Onset),
        "location"    => Some(Location),
        "duration"    => Some(Duration),
        "character"   => Some(Character),
        "aggravating" => Some(Aggravating),
        "alleviating" => Some(Alleviating),
        "radiation"   => Some(Radiation),
        "timing"      => Some(Timing),
        "severity"    => Some(Severity),
        _             => None,
    }
}
