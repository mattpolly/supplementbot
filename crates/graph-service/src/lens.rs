use crate::types::{Complexity, EdgeType, NodeType};

// ---------------------------------------------------------------------------
// ComplexityLens — a filter that determines what the NSAI can "see"
//
// The lens is a continuous value from 0.0 to 1.0. Node types and edge types
// become visible when their minimum complexity threshold is at or below the
// lens value.
//
// Named presets exist for convenience (5th grade ≈ 0.15, 10th ≈ 0.45, etc.)
// but the system operates on the float, not the name.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ComplexityLens {
    level: Complexity,
}

impl ComplexityLens {
    /// Create a lens at a specific complexity level (clamped to 0.0–1.0)
    pub fn new(level: Complexity) -> Self {
        Self {
            level: level.clamp(0.0, 1.0),
        }
    }

    /// The raw complexity value
    pub fn level(&self) -> Complexity {
        self.level
    }

    /// Is this node type visible through this lens?
    pub fn can_see_node(&self, node_type: &NodeType) -> bool {
        node_type.min_complexity() <= self.level
    }

    /// Is this edge type visible through this lens?
    pub fn can_see_edge(&self, edge_type: &EdgeType) -> bool {
        edge_type.min_complexity() <= self.level
    }

    /// All node types visible at this complexity level
    pub fn visible_node_types(&self) -> Vec<&'static NodeType> {
        NodeType::all()
            .iter()
            .filter(|nt| self.can_see_node(nt))
            .collect()
    }

    /// All edge types visible at this complexity level
    pub fn visible_edge_types(&self) -> Vec<&'static EdgeType> {
        EdgeType::all()
            .iter()
            .filter(|et| self.can_see_edge(et))
            .collect()
    }

    // ── Named presets ───────────────────────────────────────────────────

    /// 5th grade: simple effects, basic systems, what does it do?
    pub fn fifth_grade() -> Self {
        Self::new(0.15)
    }

    /// 10th grade: basic mechanisms, competition, simple regulatory concepts
    pub fn tenth_grade() -> Self {
        Self::new(0.5)
    }

    /// College sophomore: biochemistry, cascades, receptor-level detail
    pub fn college() -> Self {
        Self::new(0.8)
    }

    /// Graduate: full ontology including feedback loops and gating
    pub fn graduate() -> Self {
        Self::new(1.0)
    }
}

impl Default for ComplexityLens {
    fn default() -> Self {
        Self::fifth_grade()
    }
}

// ---------------------------------------------------------------------------
// Prompt generation helpers
//
// Build the node-type and edge-type sections of the extraction prompt
// based on what's visible through the lens.
// ---------------------------------------------------------------------------

impl ComplexityLens {
    /// Generate the "## Node types" section for an extraction prompt
    pub fn node_types_prompt(&self) -> String {
        let mut lines = Vec::new();
        for nt in self.visible_node_types() {
            let desc = match nt {
                NodeType::Ingredient => "Ingredient: the supplement itself",
                NodeType::System => "System: a body system (e.g. nervous system, muscular system)",
                NodeType::Mechanism => "Mechanism: a biological process or pathway",
                NodeType::Symptom => "Symptom: a physiological sign (e.g. muscle cramps, fatigue)",
                NodeType::Property => "Property: a therapeutic effect or quality (e.g. muscle relaxation)",
                NodeType::Substrate => "Substrate: a signaling molecule, ion, or hormone (e.g. calcium, serotonin)",
                NodeType::Receptor => "Receptor: a molecular target (e.g. NMDA receptor, calcium channel)",
            };
            lines.push(format!("- {}", desc));
        }
        lines.join("\n")
    }

    /// Generate the "## Edge types" section for an extraction prompt
    pub fn edge_types_prompt(&self) -> String {
        self.visible_edge_types()
            .iter()
            .map(|et| format!("- {}", et.prompt_description()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fifth_grade_sees_basics() {
        let lens = ComplexityLens::fifth_grade();

        assert!(lens.can_see_node(&NodeType::Ingredient));
        assert!(lens.can_see_node(&NodeType::System));
        assert!(lens.can_see_node(&NodeType::Property));
        assert!(lens.can_see_node(&NodeType::Mechanism));
        assert!(lens.can_see_node(&NodeType::Symptom));

        // Should NOT see advanced types
        assert!(!lens.can_see_node(&NodeType::Substrate));
        assert!(!lens.can_see_node(&NodeType::Receptor));
    }

    #[test]
    fn test_fifth_grade_edge_visibility() {
        let lens = ComplexityLens::fifth_grade();

        assert!(lens.can_see_edge(&EdgeType::ActsOn));
        assert!(lens.can_see_edge(&EdgeType::Affords));
        assert!(lens.can_see_edge(&EdgeType::ViaMechanism));
        assert!(lens.can_see_edge(&EdgeType::Modulates));

        // Should NOT see advanced edge types
        assert!(!lens.can_see_edge(&EdgeType::CompetesWith));
        assert!(!lens.can_see_edge(&EdgeType::Disinhibits));
        assert!(!lens.can_see_edge(&EdgeType::Amplifies));
        assert!(!lens.can_see_edge(&EdgeType::Gates));
    }

    #[test]
    fn test_tenth_grade_sees_intermediate() {
        let lens = ComplexityLens::tenth_grade();

        // All basics
        assert!(lens.can_see_edge(&EdgeType::ActsOn));
        assert!(lens.can_see_edge(&EdgeType::Modulates));

        // Plus intermediate
        assert!(lens.can_see_edge(&EdgeType::ContraindicatedWith));
        assert!(lens.can_see_edge(&EdgeType::CompetesWith));
        assert!(lens.can_see_edge(&EdgeType::Disinhibits));
        assert!(lens.can_see_node(&NodeType::Substrate));

        // But not advanced
        assert!(!lens.can_see_edge(&EdgeType::Amplifies));
        assert!(!lens.can_see_edge(&EdgeType::Gates));
        assert!(!lens.can_see_node(&NodeType::Receptor));
    }

    #[test]
    fn test_college_sees_advanced() {
        let lens = ComplexityLens::college();

        assert!(lens.can_see_edge(&EdgeType::Sequesters));
        assert!(lens.can_see_edge(&EdgeType::Releases));
        assert!(lens.can_see_edge(&EdgeType::Amplifies));
        assert!(lens.can_see_edge(&EdgeType::Desensitizes));
        assert!(lens.can_see_node(&NodeType::Receptor));

        // But not expert
        assert!(!lens.can_see_edge(&EdgeType::PositivelyReinforces));
        assert!(!lens.can_see_edge(&EdgeType::Gates));
    }

    #[test]
    fn test_graduate_sees_everything() {
        let lens = ComplexityLens::graduate();

        for nt in NodeType::all() {
            assert!(lens.can_see_node(nt), "graduate should see {:?}", nt);
        }
        for et in EdgeType::all() {
            assert!(lens.can_see_edge(et), "graduate should see {:?}", et);
        }
    }

    #[test]
    fn test_custom_level() {
        let lens = ComplexityLens::new(0.35);

        assert!(lens.can_see_edge(&EdgeType::ContraindicatedWith)); // 0.3
        assert!(!lens.can_see_edge(&EdgeType::CompetesWith)); // 0.4
    }

    #[test]
    fn test_clamps_to_range() {
        let low = ComplexityLens::new(-0.5);
        assert!((low.level() - 0.0).abs() < f64::EPSILON);

        let high = ComplexityLens::new(1.5);
        assert!((high.level() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_visible_counts_increase_with_complexity() {
        let fifth = ComplexityLens::fifth_grade();
        let tenth = ComplexityLens::tenth_grade();
        let college = ComplexityLens::college();
        let grad = ComplexityLens::graduate();

        let fifth_edges = fifth.visible_edge_types().len();
        let tenth_edges = tenth.visible_edge_types().len();
        let college_edges = college.visible_edge_types().len();
        let grad_edges = grad.visible_edge_types().len();

        assert!(fifth_edges < tenth_edges, "10th should see more than 5th");
        assert!(tenth_edges < college_edges, "college should see more than 10th");
        assert!(college_edges < grad_edges, "graduate should see more than college");
        assert_eq!(grad_edges, EdgeType::all().len(), "graduate should see all");
    }

    #[test]
    fn test_node_types_prompt() {
        let lens = ComplexityLens::fifth_grade();
        let prompt = lens.node_types_prompt();

        assert!(prompt.contains("Ingredient"));
        assert!(prompt.contains("System"));
        assert!(prompt.contains("Property"));
        assert!(!prompt.contains("Substrate"));
        assert!(!prompt.contains("Receptor"));
    }

    #[test]
    fn test_edge_types_prompt() {
        let lens = ComplexityLens::fifth_grade();
        let prompt = lens.edge_types_prompt();

        assert!(prompt.contains("acts_on"));
        assert!(prompt.contains("affords"));
        assert!(!prompt.contains("competes_with"));
        assert!(!prompt.contains("gates"));
    }
}
