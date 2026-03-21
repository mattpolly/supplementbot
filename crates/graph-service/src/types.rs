use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Confidence — type alias now, struct later when multi-LLM scoring arrives
// ---------------------------------------------------------------------------
pub type Confidence = f64;

// ---------------------------------------------------------------------------
// Node types — the nouns of the ontology
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeType {
    Ingredient,
    System,
    Mechanism,
    Symptom,
    Property,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeData {
    /// Canonical name — the unique human-readable identifier (e.g. "Magnesium", "Nervous System")
    pub name: String,
    /// Which ontology category this node belongs to
    pub node_type: NodeType,
}

impl NodeData {
    pub fn new(name: impl Into<String>, node_type: NodeType) -> Self {
        Self {
            name: name.into(),
            node_type,
        }
    }
}

impl fmt::Display for NodeData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:?})", self.name, self.node_type)
    }
}

// ---------------------------------------------------------------------------
// Edge types — the verbs of the ontology
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeType {
    /// Ingredient acts on a System
    ActsOn,
    /// Ingredient or Mechanism acts through a Mechanism
    ViaMechanism,
    /// Ingredient affords a Property or therapeutic effect
    Affords,
    /// Symptom presents in a System
    PresentsIn,
    /// Ingredient is contraindicated with another Ingredient or Mechanism
    ContraindicatedWith,
    /// Ingredient or Mechanism modulates a System or Mechanism
    Modulates,
}

impl fmt::Display for EdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdgeType::ActsOn => write!(f, "acts_on"),
            EdgeType::ViaMechanism => write!(f, "via_mechanism"),
            EdgeType::Affords => write!(f, "affords"),
            EdgeType::PresentsIn => write!(f, "presents_in"),
            EdgeType::ContraindicatedWith => write!(f, "contraindicated_with"),
            EdgeType::Modulates => write!(f, "modulates"),
        }
    }
}

// ---------------------------------------------------------------------------
// Edge metadata — what we know about a relationship
// ---------------------------------------------------------------------------

/// Where this edge's data came from
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// Directly extracted from an LLM response
    Extracted,
    /// Inferred from graph topology (speculative inference engine)
    StructurallyEmergent,
}

/// Flexible metadata value for dimension-specific data that doesn't exist yet.
/// New dimensions (dosage, delivery method, etc.) go here without schema changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    String(String),
    Float(f64),
    Bool(bool),
    Int(i64),
}

/// LLM agreement record — which providers weighed in and what they said
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmAgreement {
    /// Provider name → confidence score from that provider
    pub scores: HashMap<String, Confidence>,
}

/// Everything we know about an edge beyond its type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeMetadata {
    /// Overall confidence in this relationship (0.0–1.0)
    pub confidence: Confidence,
    /// Where this edge came from
    pub source: Source,
    /// Which bootstrap iteration created or last updated this edge
    pub iteration: u32,
    /// Ontology epoch — incremented when new dimensions are introduced.
    /// Edges from earlier epochs may need re-evaluation.
    pub epoch: u32,
    /// Per-LLM confidence scores (populated during review pipeline)
    pub llm_agreement: Option<LlmAgreement>,
    /// Open map for future dimensions (dosage, delivery method, etc.)
    #[serde(default)]
    pub extra: HashMap<String, MetadataValue>,
}

impl EdgeMetadata {
    /// Create metadata for a freshly extracted edge
    pub fn extracted(confidence: Confidence, iteration: u32, epoch: u32) -> Self {
        Self {
            confidence,
            source: Source::Extracted,
            iteration,
            epoch,
            llm_agreement: None,
            extra: HashMap::new(),
        }
    }

    /// Create metadata for a structurally emergent edge (speculative inference)
    pub fn emergent(confidence: Confidence, iteration: u32, epoch: u32) -> Self {
        Self {
            confidence,
            source: Source::StructurallyEmergent,
            iteration,
            epoch,
            llm_agreement: None,
            extra: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// EdgeData — the full edge payload stored in petgraph
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeData {
    /// The relationship type
    pub edge_type: EdgeType,
    /// Metadata about this relationship
    pub metadata: EdgeMetadata,
}

impl EdgeData {
    pub fn new(edge_type: EdgeType, metadata: EdgeMetadata) -> Self {
        Self {
            edge_type,
            metadata,
        }
    }
}

impl fmt::Display for EdgeData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [confidence: {:.2}, {:?}, epoch: {}]",
            self.edge_type, self.metadata.confidence, self.metadata.source, self.metadata.epoch
        )
    }
}
