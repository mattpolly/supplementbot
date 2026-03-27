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
    /// A named biological pathway (e.g. "calcium absorption pathway", "mevalonate pathway")
    Pathway,
    /// A named biological process (e.g. "inflammation", "oxidative phosphorylation")
    BiologicalProcess,
    /// A disease or medical condition disclosed by the user (e.g. "hemophilia", "diabetes")
    /// NEVER surfaced in recommendations — used only for contraindication safety filtering
    Condition,
    /// A biochemical intermediate distinct from a substrate (e.g. "5-HTP", "homocysteine")
    Metabolite,
    /// A gene or protein target (e.g. "MTHFR", "COX-2", "cytochrome P450")
    GeneProtein,
    /// A cell type involved in supplement interactions (e.g. "T-cell", "macrophage")
    CellType,
    /// A microorganism in the gut or body microbiome (e.g. "Lactobacillus", "Bifidobacterium")
    Microbiota,
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
            NodeType::Condition => 0.3,
            NodeType::Substrate => 0.4,
            NodeType::Pathway => 0.5,
            NodeType::BiologicalProcess => 0.5,
            NodeType::Metabolite => 0.5,
            NodeType::GeneProtein => 0.7,
            NodeType::CellType => 0.7,
            NodeType::Microbiota => 0.7,
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
            NodeType::Condition,
            NodeType::Substrate,
            NodeType::Pathway,
            NodeType::BiologicalProcess,
            NodeType::Metabolite,
            NodeType::GeneProtein,
            NodeType::CellType,
            NodeType::Microbiota,
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

        // Condition nodes can ONLY be the target of contraindicated_with.
        // This is the structural defense preventing medical claims in the graph.
        if matches!(source, Condition) || matches!(target, Condition) {
            return !matches!(self, EdgeType::ContraindicatedWith);
        }

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
            EdgeType::Modulates => "modulates: Mechanism → System or Mechanism → Mechanism (e.g. \"NMDA receptor modulation modulates nervous system\")",
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

impl std::str::FromStr for EdgeType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "acts_on" => Ok(EdgeType::ActsOn),
            "via_mechanism" => Ok(EdgeType::ViaMechanism),
            "affords" => Ok(EdgeType::Affords),
            "presents_in" => Ok(EdgeType::PresentsIn),
            "modulates" => Ok(EdgeType::Modulates),
            "contraindicated_with" => Ok(EdgeType::ContraindicatedWith),
            "competes_with" => Ok(EdgeType::CompetesWith),
            "disinhibits" => Ok(EdgeType::Disinhibits),
            "sequesters" => Ok(EdgeType::Sequesters),
            "releases" => Ok(EdgeType::Releases),
            "amplifies" => Ok(EdgeType::Amplifies),
            "desensitizes" => Ok(EdgeType::Desensitizes),
            "positively_reinforces" => Ok(EdgeType::PositivelyReinforces),
            "gates" => Ok(EdgeType::Gates),
            other => Err(format!("unknown edge type: \"{}\"", other)),
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
    /// How many layers of reasoning produced this edge.
    /// 0 = directly extracted from LLM, 1 = deduced from extracted edges,
    /// 2 = speculated from deduced edges, etc. Used to prevent
    /// speculation-on-deduction cascades.
    #[serde(default)]
    pub reasoning_depth: u32,
    /// Open map for future dimensions (dosage, delivery method, etc.)
    #[serde(default)]
    pub extra: HashMap<String, MetadataValue>,
}

impl EdgeMetadata {
    /// Create metadata for a freshly extracted edge (depth 0)
    pub fn extracted(confidence: Confidence, iteration: u32, epoch: u32) -> Self {
        Self {
            confidence,
            source: Source::Extracted,
            iteration,
            epoch,
            llm_agreement: None,
            reasoning_depth: 0,
            extra: HashMap::new(),
        }
    }

    /// Create metadata for a structurally emergent edge (speculative inference).
    /// Depth is 1 by default — speculated from extracted premises.
    pub fn emergent(confidence: Confidence, iteration: u32, epoch: u32) -> Self {
        Self {
            confidence,
            source: Source::StructurallyEmergent,
            iteration,
            epoch,
            llm_agreement: None,
            reasoning_depth: 1,
            extra: HashMap::new(),
        }
    }

    /// Create metadata for a deduced edge (symbolic forward chaining).
    /// `premise_depth` should be the max reasoning_depth of the premise edges.
    pub fn deduced(confidence: Confidence, iteration: u32, epoch: u32) -> Self {
        Self {
            confidence,
            source: Source::Deduced,
            iteration,
            epoch,
            llm_agreement: None,
            reasoning_depth: 1,
            extra: HashMap::new(),
        }
    }

    /// Create deduced metadata with an explicit depth derived from premises.
    pub fn deduced_with_depth(
        confidence: Confidence,
        iteration: u32,
        epoch: u32,
        premise_max_depth: u32,
    ) -> Self {
        Self {
            confidence,
            source: Source::Deduced,
            iteration,
            epoch,
            llm_agreement: None,
            reasoning_depth: premise_max_depth + 1,
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
