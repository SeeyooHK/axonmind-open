use serde_json::{Value, json};

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "focus_kpi",
            description: "Return a KPI node with its drivers, blockers, risks, owner, and evidence count.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kpi_id": { "type": "string", "description": "The node id of the KPI, e.g. \"kpi.revenue_growth\"." }
                },
                "required": ["kpi_id"]
            }),
        },
        ToolDef {
            name: "explain_kpi",
            description: "Return a rationale built from evidence quotes, the supporting evidence list, and a confidence score for a KPI.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kpi_id": { "type": "string" },
                    "depth": { "type": "integer", "description": "How many driver/blocker hops to include. Default: 2." }
                },
                "required": ["kpi_id"]
            }),
        },
        ToolDef {
            name: "get_evidence",
            description: "Fetch all evidence items for a node or edge. Provide exactly one of node_id or edge_id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" },
                    "edge_id": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "impact_radius",
            description: "Traverse the graph outward from a node and return all reachable affected nodes with their depth and path.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node_id": { "type": "string" },
                    "max_depth": { "type": "integer", "description": "Maximum traversal depth. Default: 3." }
                },
                "required": ["node_id"]
            }),
        },
        ToolDef {
            name: "trace_decision",
            description: "Return a decision node with its causal predecessors, supporting evidence, and next actions.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "decision_node_id": { "type": "string" }
                },
                "required": ["decision_node_id"]
            }),
        },
        ToolDef {
            name: "suggest_actions",
            description: "Return action nodes linked to a KPI, optionally filtered by KPI status.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kpi_id": { "type": "string" },
                    "status_filter": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["Healthy", "AtRisk", "Critical", "Unknown"]
                        },
                        "description": "Only suggest actions relevant to these KPI statuses. Default: all."
                    },
                    "include_unreviewed": {
                        "type": "boolean",
                        "description": "Include actions linked only through review-required edges. Default: false."
                    }
                },
                "required": ["kpi_id"]
            }),
        },
        ToolDef {
            name: "graph_search",
            description: "Full-text search the knowledge graph via FTS5. Returns matching nodes and which fields matched.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "kinds": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": [
                                "Kpi","Metric","Objective","Initiative","Risk",
                                "Opportunity","Decision","Insight","Document",
                                "Person","Team","Customer","Function","Product",
                                "Market","Process","System","Action"
                            ]
                        },
                        "description": "Filter by node kind. Default: all kinds."
                    },
                    "limit": { "type": "integer", "description": "Maximum results. Default: 20." }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "reasoning_search",
            description: "Vectorless two-stage retrieval: BM25 recall then LLM reasoning precision over indexed document sections.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "doc_node_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict to specific document node ids. Default: whole corpus."
                    },
                    "max_results": { "type": "integer", "description": "Maximum results. Default: 20." }
                },
                "required": ["query"]
            }),
        },
    ]
}
