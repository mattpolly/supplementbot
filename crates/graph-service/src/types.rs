use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use surrealdb_types::SurrealValue;

// ---------------------------------------------------------------------------
// Confidence — type alias now, struct later when multi-LLM scoring arrives
// ---------------------------------------------------------------------------
pub type Confidence = f64;

// ---------------------------------------------------------------------------
// Complexity — continuous dial for ontology visibility
//
// 0.0 = simplest (5th grade: "what does it do?")
// 1.0 = full biochemistry (graduate level: cascades, feedback loops, gating)
//
// Every node type and edge type has a minimum complexity threshold.
// A ComplexityLens at level X can see anything with threshold <= X.
// ---------------------------------------------------------------------------
pub type Complexity = f64;

// ---------------------------------------------------------------------------
// Node types — the nouns of the ontology
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, SurrealValue)]
#[serde(tag = "kind")]
pub enum NodeType {
    /// A supplement or nutraceutical (e.g. "Magnesium")
    Ingredient,
    /// A body system (e.g. "nervous system", "muscular system")
    System,
    /// A biological process or pathway (e.g. "calcium channel blocking")
    Mechanism,
    /// A physiological sign (e.g. "muscle cramps", "fatigue")
    Symptom,
    /// A therapeutic effect or quality (e.g. "muscle relaxation", "sleep quality")
    Property,
    /// A signaling molecule, ion, or hormone (e.g. "calcium", "serotonin", "cortisol")
    Substrate,
    /// A molecular target (e.g. "NMDA receptor", "L-type calcium channel")
    Receptor,
}

impl NodeType {
    /// Minimum complexity level at which this node type becomes visible
    pub fn min_complexity(&self) -> Complexity {
        match self {
            NodeType::Ingredient => 0.0,
            NodeType::System => 0.0,
            NodeType::Mechanism => 0.0,
            NodeType::Symptom => 0.0,
            NodeType::Property => 0.0,
            NodeType::Substrate => 0.4,
            NodeType::Receptor => 0.7,
        }
    }

    /// All node types, ordered by complexity threshold
    pub fn all() -> &'static [NodeType] {
        &[
            NodeType::Ingredient,
            NodeType::System,
            NodeType::Mechanism,
            NodeType::Symptom,
            NodeType::Property,
            NodeType::Substrate,
            NodeType::Receptor,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, SurrealValue)]
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
//
// Organized by complexity threshold. Foundational types (0.0) are visible
// at all levels. Advanced regulatory forces require higher complexity.
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, SurrealValue)]
pub enum EdgeType {
    // ── Foundational (0.0) — visible at all levels ──────────────────────
    /// Ingredient acts on a System
    ActsOn,
    /// Ingredient or Mechanism acts through a Mechanism
    ViaMechanism,
    /// Ingredient affords a Property or therapeutic effect
    Affords,
    /// Symptom presents in a System
    PresentsIn,
    /// Ingredient or Mechanism modulates (gain control) a System or Mechanism
    Modulates,

    // ── Intermediate (0.3–0.5) — 10th grade biology ─────────────────────
    /// Ingredient is contraindicated with another Ingredient or Mechanism
    ContraindicatedWith,
    /// Molecules compete for the same binding site (concentration-dependent)
    CompetesWith,
    /// Removing an inhibitor increases downstream activity
    Disinhibits,

    // ── Advanced (0.6–0.8) — college biochemistry ───────────────────────
    /// Stores/sequesters a substrate for later release
    Sequesters,
    /// Releases a sequestered substrate on demand
    Releases,
    /// Nonlinear amplification through enzymatic cascades
    Amplifies,
    /// Prolonged stimulation reduces receptor sensitivity
    Desensitizes,

    // ── Expert (0.85–1.0) — graduate-level regulatory logic ─────────────
    /// Output feeds back to increase its own input (runaway until terminated)
    PositivelyReinforces,
    /// All-or-nothing activation above a threshold
    Gates,
}

impl EdgeType {
    /// Minimum complexity level at which this edge type becomes visible
    pub fn min_complexity(&self) -> Complexity {
        match self {
            // Foundational — a 5th grader can understand these
            EdgeType::ActsOn => 0.0,
            EdgeType::ViaMechanism => 0.0,
            EdgeType::Affords => 0.0,
            EdgeType::PresentsIn => 0.0,
            EdgeType::Modulates => 0.0,

            // Intermediate — 10th grade biology
            EdgeType::ContraindicatedWith => 0.3,
            EdgeType::CompetesWith => 0.4,
            EdgeType::Disinhibits => 0.5,

            // Advanced — college biochemistry
            EdgeType::Sequesters => 0.6,
            EdgeType::Releases => 0.6,
            EdgeType::Amplifies => 0.7,
            EdgeType::Desensitizes => 0.7,

            // Expert — graduate-level
            EdgeType::PositivelyReinforces => 0.85,
            EdgeType::Gates => 0.9,
        }
    }

    /// All edge types, ordered by complexity threshold
    pub fn all() -> &'static [EdgeType] {
        &[
            EdgeType::ActsOn,
            EdgeType::ViaMechanism,
            EdgeType::Affords,
            EdgeType::PresentsIn,
            EdgeType::Modulates,
            EdgeType::ContraindicatedWith,
            EdgeType::CompetesWith,
            EdgeType::Disinhibits,
            EdgeType::Sequesters,
            EdgeType::Releases,
            EdgeType::Amplifies,
            EdgeType::Desensitizes,
            EdgeType::PositivelyReinforces,
            EdgeType::Gates,
        ]
    }

    /// Pairs that are clearly wrong for this edge type (denylist).
    /// Returns true if this (source, target) combination should be rejected.
    ///
    /// We use a denylist rather than an allowlist because the LLMs are
    /// inconsistent about node typing (e.g. "energy production" as Mechanism
    /// vs Property). Strict allowlists reject too many valid triples.
    /// The denylist catches only the semantically nonsensical cases.
    pub fn is_invalid_pair(&self, source: &NodeType, target: &NodeType) -> bool {
        use NodeType::*;
        match self {
            // presents_in is strictly Symptom → System
            EdgeType::PresentsIn => {
                !matches!(source, Symptom) || !matches!(target, System)
            }
            // acts_on should have Ingredient as source, System as target
            EdgeType::ActsOn => {
                !matches!(source, Ingredient) || !matches!(target, System)
            }
            // Everything else: allow unless it's obviously wrong
            _ => false,
        }
    }

    /// Check if a (source_type, target_type) pair is valid for this edge type
    pub fn is_valid_pair(&self, source: &NodeType, target: &NodeType) -> bool {
        !self.is_invalid_pair(source, target)
    }

    /// Simple description suitable for an LLM extraction prompt
    pub fn prompt_description(&self) -> &'static str {
        match self {
            EdgeType::ActsOn => "acts_on: Ingredient → System",
            EdgeType::ViaMechanism => "via_mechanism: Ingredient → Mechanism, or Mechanism → Mechanism",
            EdgeType::Affords => "affords: Ingredient → Property, or Mechanism → Property",
            EdgeType::PresentsIn => "presents_in: Symptom → System",
            EdgeType::Modulates => "modulates: adjusts sensitivity of a System or Mechanism (gain control)",
            EdgeType::ContraindicatedWith => "contraindicated_with: should not be combined with",
            EdgeType::CompetesWith => "competes_with: molecules competing for the same binding site",
            EdgeType::Disinhibits => "disinhibits: removes an inhibitor, increasing downstream activity",
            EdgeType::Sequesters => "sequesters: stores a molecule for later release",
            EdgeType::Releases => "releases: releases a stored molecule on demand",
            EdgeType::Amplifies => "amplifies: small input produces large output (cascade)",
            EdgeType::Desensitizes => "desensitizes: prolonged exposure reduces sensitivity",
            EdgeType::PositivelyReinforces => "positively_reinforces: output feeds back to increase its own input",
            EdgeType::Gates => "gates: all-or-nothing activation above a threshold",
        }
    }
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
            EdgeType::CompetesWith => write!(f, "competes_with"),
            EdgeType::Disinhibits => write!(f, "disinhibits"),
            EdgeType::Sequesters => write!(f, "sequesters"),
            EdgeType::Releases => write!(f, "releases"),
            EdgeType::Amplifies => write!(f, "amplifies"),
            EdgeType::Desensitizes => write!(f, "desensitizes"),
            EdgeType::PositivelyReinforces => write!(f, "positively_reinforces"),
            EdgeType::Gates => write!(f, "gates"),
        }
    }
}

// ---------------------------------------------------------------------------
// Edge metadata — what we know about a relationship
// ---------------------------------------------------------------------------

/// Where this edge's data came from
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, SurrealValue)]
pub enum Source {
    /// Directly extracted from an LLM response
    Extracted,
    /// Inferred from graph topology (speculative inference engine)
    StructurallyEmergent,
    /// Deduced by symbolic forward chaining (guaranteed given premises)
    Deduced,
}

/// Flexible metadata value for dimension-specific data that doesn't exist yet.
/// New dimensions (dosage, delivery method, etc.) go here without schema changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
#[serde(untagged)]
pub enum MetadataValue {
    String(String),
    Float(f64),
    Bool(bool),
    Int(i64),
}

/// LLM agreement record — which providers weighed in and what they said
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
pub struct LlmAgreement {
    /// Provider name → confidence score from that provider
    pub scores: HashMap<String, Confidence>,
}

/// Everything we know about an edge beyond its type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
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

    /// Create metadata for a deduced edge (symbolic forward chaining)
    pub fn deduced(confidence: Confidence, iteration: u32, epoch: u32) -> Self {
        Self {
            confidence,
            source: Source::Deduced,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SurrealValue)]
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
