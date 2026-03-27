use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Envelope — every event gets wrapped with correlation + timing metadata
// ---------------------------------------------------------------------------

/// A timestamped, correlated event. The correlation_id ties together all events
/// from a single bootstrap iteration for a single nutraceutical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    /// Unique ID for this specific event
    pub event_id: Uuid,
    /// Groups related events (e.g. one bootstrap iteration for "Magnesium")
    pub correlation_id: Uuid,
    /// When this event was emitted
    pub timestamp: DateTime<Utc>,
    /// The actual event payload
    pub event: PipelineEvent,
}

impl EventEnvelope {
    pub fn new(correlation_id: Uuid, event: PipelineEvent) -> Self {
        Self {
            event_id: Uuid::new_v4(),
            correlation_id,
            timestamp: Utc::now(),
            event,
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline events — typed representation of every data exchange
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PipelineEvent {
    // -- LLM interactions (Phase 2) ---------------------------------------

    /// An LLM request is about to be sent
    LlmRequest {
        provider: String,
        model: String,
        prompt: String,
        nutraceutical: String,
        stage: CurriculumStage,
        question_type: String,
    },

    /// An LLM response was received
    LlmResponse {
        provider: String,
        model: String,
        raw_response: String,
        latency_ms: u64,
        tokens_used: Option<TokenUsage>,
    },

    /// An LLM call failed
    LlmError {
        provider: String,
        model: String,
        error: String,
    },

    // -- Extraction (Phase 3) ---------------------------------------------

    /// Raw LLM response entering the extraction parser
    ExtractionInput {
        raw_response: String,
        nutraceutical: String,
        stage: CurriculumStage,
    },

    /// Structured output from the extraction parser
    ExtractionOutput {
        nodes_added: Vec<NodeRef>,
        edges_added: Vec<EdgeRef>,
        parse_warnings: Vec<String>,
    },

    // -- NSAI loop (Phase 3) -----------------------------------------------

    /// Graph analysis identified gaps to fill
    GapAnalysis {
        gaps: Vec<GapInfo>,
        graph_nodes: usize,
        graph_edges: usize,
    },

    /// Comprehension check: NSAI rephrased its understanding
    ComprehensionCheck {
        rephrase_prompt: String,
        rephrase_response: String,
        edges_confirmed: usize,
        edges_new: usize,
        edges_total: usize,
    },

    /// One iteration of the NSAI loop completed
    LoopIteration {
        iteration: u32,
        phase: String,
        gaps_found: usize,
        nodes_before: usize,
        nodes_after: usize,
        edges_before: usize,
        edges_after: usize,
    },

    // -- Speculative inference (Phase 4) ----------------------------------

    /// A candidate claim generated from graph topology
    SpeculativeClaim {
        claim: String,
        topology_justification: String,
        source_nodes: Vec<String>,
    },

    // -- Forward chaining (symbolic deduction) ------------------------------

    /// A deduction produced by symbolic forward chaining
    ForwardChain {
        rule: String,
        premise_a: String,
        premise_b: String,
        conclusion: String,
        confidence: f64,
    },

    // -- Review pipeline (Phase 5) ----------------------------------------

    /// Result of multi-LLM review of a speculative claim
    ReviewResult {
        claim: String,
        provider_scores: Vec<ProviderScore>,
        final_confidence: f64,
        verdict: ReviewVerdict,
    },

    // -- Graph mutations (any phase) --------------------------------------

    /// A node was added or updated in the knowledge graph
    GraphNodeMutation {
        operation: MutationOp,
        node_name: String,
        node_type: String,
    },

    /// An edge was added or updated in the knowledge graph
    GraphEdgeMutation {
        operation: MutationOp,
        source_node: String,
        target_node: String,
        edge_type: String,
        confidence: f64,
        /// How this edge was produced (Extracted, StructurallyEmergent, Deduced)
        #[serde(default)]
        source_tag: Option<String>,
        /// Which provider produced this edge
        #[serde(default)]
        provider: Option<String>,
        /// Which model produced this edge
        #[serde(default)]
        model: Option<String>,
    },

    /// An existing edge was independently re-observed (confirmation signal)
    EdgeConfirmed {
        source_node: String,
        target_node: String,
        edge_type: String,
        /// Which provider confirmed this edge
        provider: String,
        /// Which model confirmed this edge
        model: String,
    },

    // -- Synonym resolution ------------------------------------------------

    /// Synonym resolution pass results
    SynonymResolution {
        /// How many nodes were matched to CUIs
        cuis_assigned: usize,
        /// How many alias pairs were detected
        aliases_found: usize,
    },

    // -- Citation backing --------------------------------------------------

    /// Citation backing pass results
    CitationBacking {
        /// Number of graph edges checked for SuppKG matches
        edges_checked: usize,
        /// Number of edges that received at least one citation
        edges_backed: usize,
        /// Total PubMed citations stored
        citations_stored: usize,
        /// Sample of citations (edge → PMID) for the log
        sample: Vec<CitationRef>,
    },
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CurriculumStage {
    /// Stage 1: basic systems, mechanisms, therapeutic uses
    Foundational,
    /// Stage 2: cross-system links, contraindications, interactions
    Relational,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Lightweight reference to a node (for logging, not the actual graph node)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRef {
    pub name: String,
    pub node_type: String,
}

/// Lightweight reference to an edge (for logging, not the actual graph edge)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeRef {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub confidence: f64,
}

/// A gap identified by graph analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapInfo {
    pub node_name: String,
    pub gap_type: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderScore {
    pub provider: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewVerdict {
    /// LLMs agree and can cite mechanisms
    Confirmed,
    /// Reasonable but unconfirmed — the discovery track
    Plausible,
    /// LLMs disagree
    Contested,
    /// LLMs reject the claim
    Rejected,
}

/// A citation reference for event logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationRef {
    pub source_node: String,
    pub target_node: String,
    pub edge_type: String,
    pub pmid: String,
    pub suppkg_predicate: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MutationOp {
    Added,
    Updated,
    Removed,
}
