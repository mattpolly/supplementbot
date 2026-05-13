use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Read-only database explorer endpoints
//
// All data now served from supplementology (Postgres) via SupplementClient.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct PageQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

fn default_limit() -> usize {
    50
}

// ---------------------------------------------------------------------------
// Graph explorer endpoints
// ---------------------------------------------------------------------------

/// GET /api/explore/graph/nodes?limit=50&offset=0
pub async fn graph_nodes(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let api = state.inner.graph.api();
    Json(api.explore_nodes(page.limit.min(200), page.offset).await)
}

/// GET /api/explore/graph/edges?limit=50&offset=0
pub async fn graph_edges(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let api = state.inner.graph.api();
    Json(api.explore_edges(page.limit.min(200), page.offset).await)
}

/// GET /api/explore/graph/node-aliases?limit=50&offset=0
pub async fn graph_node_aliases(
    State(state): State<AppState>,
    Query(_page): Query<PageQuery>,
) -> Json<Value> {
    let aliases = state.inner.merge.all_aliases().await;
    let rows: Vec<Value> = aliases.iter().map(|a| json!({
        "canonical":  a.canonical,
        "alias":      a.alias,
        "confidence": a.confidence,
        "method":     a.method,
        "created_at": a.created_at,
    })).collect();
    Json(json!({ "rows": rows }))
}

/// GET /api/explore/graph/node-cuis?limit=50&offset=0
pub async fn graph_node_cuis(
    State(state): State<AppState>,
    Query(_page): Query<PageQuery>,
) -> Json<Value> {
    let cuis = state.inner.merge.all_cuis().await;
    let rows: Vec<Value> = cuis.iter().map(|c| json!({
        "node_name":  c.node_name,
        "cui":        c.cui,
        "confidence": c.confidence,
        "method":     c.method,
    })).collect();
    Json(json!({ "rows": rows }))
}

/// GET /api/explore/graph/edge-sources?limit=50&offset=0
pub async fn graph_edge_sources(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let api = state.inner.graph.api();
    Json(api.explore_edge_sources(page.limit.min(200), page.offset).await)
}

/// GET /api/explore/graph/edge-citations?limit=50&offset=0
pub async fn graph_edge_citations(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let api = state.inner.graph.api();
    Json(api.explore_edge_citations(page.limit.min(200), page.offset).await)
}

/// GET /api/explore/graph/stats — counts for all graph tables
pub async fn graph_stats(State(state): State<AppState>) -> Json<Value> {
    let api = state.inner.graph.api();
    let mut stats = api.graph_stats().await;

    // Merge node_alias and node_cui counts (from Postgres merge tables)
    let node_aliases = state.inner.merge.alias_count().await;
    let node_cuis = state.inner.merge.all_cuis().await.len();
    if let Some(obj) = stats.as_object_mut() {
        obj.insert("node_aliases".to_string(), json!(node_aliases));
        obj.insert("node_cuis".to_string(), json!(node_cuis));
    }

    Json(stats)
}

// ---------------------------------------------------------------------------
// Relational explorer endpoints (intake_ tables — all from PgIntakeStore)
// ---------------------------------------------------------------------------

/// GET /api/explore/relational/stats — counts for intake + iDISK tables
pub async fn relational_stats(State(state): State<AppState>) -> Json<Value> {
    let intake = &state.inner.intake_store;
    let mut stats = serde_json::Map::new();

    stats.insert("intake_archetypes".to_string(), json!(intake.all_archetypes().len()));
    stats.insert("intake_symptom_profiles".to_string(), json!(intake.all_symptom_profile_ids().len()));
    stats.insert("intake_questions".to_string(), json!(intake.question_count()));
    stats.insert("intake_clusters".to_string(), json!(intake.cluster_count()));

    Json(Value::Object(stats))
}

/// GET /api/explore/relational/intake-stages
pub async fn intake_stages(_state: State<AppState>) -> Json<Value> {
    let empty: Vec<Value> = vec![];
    Json(json!({ "rows": empty, "note": "stages are now embedded in the engine" }))
}

/// GET /api/explore/relational/intake-archetypes
pub async fn intake_archetypes(State(state): State<AppState>) -> Json<Value> {
    let intake = &state.inner.intake_store;
    let rows: Vec<Value> = intake.all_archetypes().iter().map(|a| json!({
        "id":                    a.id,
        "name":                  a.name,
        "sufficient_dimensions": a.sufficient_dimensions,
        "relevant_oldcarts":     a.relevant_oldcarts.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
        "irrelevant_oldcarts":   a.irrelevant_oldcarts.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
        "default_systems":       a.default_systems,
    })).collect();
    Json(json!({ "rows": rows }))
}

/// GET /api/explore/relational/intake-symptom-profiles?limit=50&offset=0
pub async fn intake_symptom_profiles(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let intake = &state.inner.intake_store;
    let all_ids = intake.all_symptom_profile_ids();
    let limit = page.limit.min(200);
    let offset = page.offset;
    let rows: Vec<Value> = all_ids.iter()
        .skip(offset)
        .take(limit)
        .filter_map(|id| intake.get_symptom_profile(id))
        .map(|p| json!({
            "id":                p.id,
            "name":              p.name,
            "cui":               p.cui,
            "aliases":           p.aliases,
            "archetype_id":      p.archetype_id,
            "associated_systems": p.associated_systems,
        }))
        .collect();
    Json(json!({ "rows": rows, "limit": limit, "offset": offset }))
}

/// GET /api/explore/relational/intake-questions?limit=50&offset=0
pub async fn intake_questions(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let intake = &state.inner.intake_store;
    let all = intake.all_questions();
    let limit = page.limit.min(200);
    let offset = page.offset;
    let rows: Vec<Value> = all.iter()
        .skip(offset)
        .take(limit)
        .map(|q| json!({
            "id":                 q.id,
            "template":           q.template,
            "oldcarts_dimension": q.oldcarts_dimension.map(|d| d.as_str()),
            "system_name":        q.system_name,
        }))
        .collect();
    Json(json!({ "rows": rows, "limit": limit, "offset": offset }))
}

/// GET /api/explore/relational/intake-clusters
pub async fn intake_clusters(State(state): State<AppState>) -> Json<Value> {
    let intake = &state.inner.intake_store;
    let rows: Vec<Value> = intake.all_clusters().iter().map(|c| json!({
        "id":                  c.id,
        "name":                c.name,
        "description":         c.description,
        "member_symptoms":     c.member_symptoms,
        "prioritized_systems": c.prioritized_systems,
    })).collect();
    Json(json!({ "rows": rows }))
}

/// GET /api/explore/relational/idisk-ingredients?limit=50&offset=0
pub async fn idisk_ingredients(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let nodes = state.inner.graph.api().nodes_by_type(&graph_service::types::NodeType::Ingredient).await;
    let limit = page.limit.min(200);
    let offset = page.offset;
    let rows: Vec<Value> = nodes.iter().skip(offset).take(limit).map(|idx| {
        json!({ "id": idx.0 })
    }).collect();
    Json(json!({ "rows": rows, "limit": limit, "offset": offset }))
}

/// GET /api/explore/relational/idisk-drugs?limit=50&offset=0
pub async fn idisk_drugs(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let api = state.inner.graph.api();
    let limit = page.limit.min(200);
    let offset = page.offset;
    // Use the known_ingredients list filtered to drug type via explore_nodes
    let result = api.explore_nodes(limit, offset).await;
    Json(result)
}

/// GET /api/explore/relational/idisk-interactions?limit=50&offset=0
pub async fn idisk_interactions(
    State(state): State<AppState>,
    Query(page): Query<PageQuery>,
) -> Json<Value> {
    let api = state.inner.graph.api();
    Json(api.explore_edges(page.limit.min(200), page.offset).await)
}

/// GET /api/explore/relational/idisk-adverse?limit=50&offset=0
pub async fn idisk_adverse(
    State(_state): State<AppState>,
    Query(_page): Query<PageQuery>,
) -> Json<Value> {
    Json(json!({ "rows": [], "note": "adverse reactions now in entity/relationship tables — use /rpc/nodes_by_type?type_name=symptom" }))
}
