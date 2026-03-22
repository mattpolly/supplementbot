use serde::Serialize;

// ---------------------------------------------------------------------------
// Export types — JSON-serializable graph for visualization
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ExportGraph {
    pub nodes: Vec<ExportNode>,
    pub edges: Vec<ExportEdge>,
}

#[derive(Debug, Serialize)]
pub struct ExportNode {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub node_type: String,
}

#[derive(Debug, Serialize)]
pub struct ExportEdge {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub confidence: f64,
    pub source_tag: String,
}

// ---------------------------------------------------------------------------
// Self-contained D3.js HTML template
// ---------------------------------------------------------------------------

pub const D3_HTML_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>supplementbot — knowledge graph</title>
<script src="https://d3js.org/d3.v7.min.js"></script>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body { background: #1a1a2e; overflow: hidden; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
  svg { width: 100vw; height: 100vh; }
  .legend { position: fixed; top: 16px; left: 16px; background: rgba(26,26,46,0.92); border: 1px solid #333; border-radius: 8px; padding: 12px 16px; color: #ccc; font-size: 13px; }
  .legend h3 { color: #eee; margin-bottom: 8px; font-size: 14px; }
  .legend-item { display: flex; align-items: center; margin: 4px 0; }
  .legend-swatch { width: 12px; height: 12px; border-radius: 50%; margin-right: 8px; }
  .stats { position: fixed; bottom: 16px; left: 16px; color: #666; font-size: 12px; }
  .edge-label { font-size: 9px; fill: #666; pointer-events: none; }
  .node-label { font-size: 11px; fill: #ddd; pointer-events: none; text-anchor: middle; dominant-baseline: central; }
</style>
</head>
<body>
<div class="legend">
  <h3>supplementbot graph</h3>
  <div class="legend-item"><div class="legend-swatch" style="background:#e74c3c"></div>Ingredient</div>
  <div class="legend-item"><div class="legend-swatch" style="background:#3498db"></div>System</div>
  <div class="legend-item"><div class="legend-swatch" style="background:#9b59b6"></div>Property</div>
  <div class="legend-item"><div class="legend-swatch" style="background:#2ecc71"></div>Mechanism</div>
  <div class="legend-item"><div class="legend-swatch" style="background:#e67e22"></div>Symptom</div>
  <div style="margin-top:8px; border-top:1px solid #333; padding-top:8px;">
    <div class="legend-item" style="font-size:11px;color:#888;">— solid = Extracted</div>
    <div class="legend-item" style="font-size:11px;color:#888;">--- dashed = Speculative</div>
  </div>
</div>
<div class="stats" id="stats"></div>
<svg>
  <defs>
    <marker id="arrow" viewBox="0 0 10 6" refX="28" refY="3" markerWidth="8" markerHeight="6" orient="auto-start-reverse">
      <path d="M0,0 L10,3 L0,6 Z" fill="#555"/>
    </marker>
  </defs>
</svg>
<script>
const GRAPH = /*__GRAPH_DATA__*/{"nodes":[],"edges":[]};

const colorMap = {
  Ingredient: "#e74c3c",
  System: "#3498db",
  Property: "#9b59b6",
  Mechanism: "#2ecc71",
  Symptom: "#e67e22",
  Substrate: "#f1c40f",
  Receptor: "#1abc9c",
};

const sizeMap = {
  Ingredient: 14,
  System: 11,
  Property: 9,
  Mechanism: 9,
  Symptom: 8,
};

document.getElementById("stats").textContent =
  `${GRAPH.nodes.length} nodes · ${GRAPH.edges.length} edges`;

const width = window.innerWidth;
const height = window.innerHeight;

const svg = d3.select("svg");
const g = svg.append("g");

// Zoom
svg.call(d3.zoom().scaleExtent([0.2, 5]).on("zoom", (e) => g.attr("transform", e.transform)));

const simulation = d3.forceSimulation(GRAPH.nodes)
  .force("link", d3.forceLink(GRAPH.edges).id(d => d.id).distance(120))
  .force("charge", d3.forceManyBody().strength(-400))
  .force("center", d3.forceCenter(width / 2, height / 2))
  .force("collision", d3.forceCollide().radius(30));

// Edges
const link = g.append("g")
  .selectAll("line")
  .data(GRAPH.edges)
  .join("line")
  .attr("stroke", "#555")
  .attr("stroke-width", d => 0.5 + d.confidence * 1.5)
  .attr("stroke-opacity", d => 0.3 + d.confidence * 0.5)
  .attr("stroke-dasharray", d => d.source_tag === "StructurallyEmergent" ? "5,4" : null)
  .attr("marker-end", "url(#arrow)");

// Edge labels
const edgeLabel = g.append("g")
  .selectAll("text")
  .data(GRAPH.edges)
  .join("text")
  .attr("class", "edge-label")
  .text(d => d.edge_type);

// Nodes
const node = g.append("g")
  .selectAll("circle")
  .data(GRAPH.nodes)
  .join("circle")
  .attr("r", d => sizeMap[d.type] || 8)
  .attr("fill", d => colorMap[d.type] || "#888")
  .attr("stroke", "#fff")
  .attr("stroke-width", 1.5)
  .call(drag(simulation));

// Node labels
const label = g.append("g")
  .selectAll("text")
  .data(GRAPH.nodes)
  .join("text")
  .attr("class", "node-label")
  .attr("dy", d => (sizeMap[d.type] || 8) + 14)
  .text(d => d.name);

// Hover highlight
node.on("mouseover", function(event, d) {
  const connected = new Set();
  GRAPH.edges.forEach(e => {
    const sid = typeof e.source === "object" ? e.source.id : e.source;
    const tid = typeof e.target === "object" ? e.target.id : e.target;
    if (sid === d.id) connected.add(tid);
    if (tid === d.id) connected.add(sid);
  });
  connected.add(d.id);
  node.attr("opacity", n => connected.has(n.id) ? 1 : 0.15);
  link.attr("opacity", e => (e.source.id === d.id || e.target.id === d.id) ? 1 : 0.05);
  label.attr("opacity", n => connected.has(n.id) ? 1 : 0.1);
  edgeLabel.attr("opacity", e => (e.source.id === d.id || e.target.id === d.id) ? 1 : 0.05);
}).on("mouseout", function() {
  node.attr("opacity", 1);
  link.attr("opacity", d => 0.3 + d.confidence * 0.5);
  label.attr("opacity", 1);
  edgeLabel.attr("opacity", 1);
});

simulation.on("tick", () => {
  link
    .attr("x1", d => d.source.x).attr("y1", d => d.source.y)
    .attr("x2", d => d.target.x).attr("y2", d => d.target.y);
  edgeLabel
    .attr("x", d => (d.source.x + d.target.x) / 2)
    .attr("y", d => (d.source.y + d.target.y) / 2);
  node.attr("cx", d => d.x).attr("cy", d => d.y);
  label.attr("x", d => d.x).attr("y", d => d.y);
});

function drag(sim) {
  return d3.drag()
    .on("start", (e, d) => { if (!e.active) sim.alphaTarget(0.3).restart(); d.fx = d.x; d.fy = d.y; })
    .on("drag", (e, d) => { d.fx = e.x; d.fy = e.y; })
    .on("end", (e, d) => { if (!e.active) sim.alphaTarget(0); d.fx = null; d.fy = null; });
}
</script>
</body>
</html>
"##;
