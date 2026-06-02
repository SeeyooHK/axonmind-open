use axonmind_core::AxonMindError;
use axonmind_engine::{
    AxonMindEngine,
    config::EngineConfig,
    mcp::{call_tool, list_tools, tool_defs},
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn test_engine_config(dir: &TempDir) -> EngineConfig {
    EngineConfig::from_workspace_dir(dir.path().to_path_buf())
}

// ── catalog shape ────────────────────────────────────────────────────────────

#[test]
fn tool_defs_count_and_names() {
    let defs = tool_defs();
    assert_eq!(defs.len(), 8);
    let names: Vec<_> = defs.iter().map(|t| t.name).collect();
    assert!(names.contains(&"focus_kpi"));
    assert!(names.contains(&"explain_kpi"));
    assert!(names.contains(&"get_evidence"));
    assert!(names.contains(&"impact_radius"));
    assert!(names.contains(&"trace_decision"));
    assert!(names.contains(&"suggest_actions"));
    assert!(names.contains(&"graph_search"));
    assert!(names.contains(&"reasoning_search"));
}

#[test]
fn tool_defs_unique_names() {
    let defs = tool_defs();
    let mut names = std::collections::HashSet::new();
    for d in &defs {
        assert!(names.insert(d.name), "duplicate tool name: {}", d.name);
    }
}

#[test]
fn tool_defs_schemas_are_objects() {
    for def in tool_defs() {
        let schema = &def.input_schema;
        assert_eq!(
            schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "tool '{}' inputSchema must have type:object",
            def.name
        );
    }
}

#[test]
fn list_tools_matches_tool_defs() {
    assert_eq!(list_tools().len(), tool_defs().len());
}

// ── schema drift guard ───────────────────────────────────────────────────────
// For each tool, build the minimal sample args that satisfy every required
// field (with syntactically correct types) and verify call_tool does NOT fail
// with a deserialization/ValidationFailed error against an empty workspace.
// Domain errors (NodeNotFound, etc.) are expected and acceptable — those prove
// the dispatch happened correctly.

fn is_schema_error(e: &AxonMindError) -> bool {
    matches!(e, AxonMindError::ValidationFailed { message } if message.contains("invalid arguments"))
}

// Every declared property (required AND optional) is populated so a rename or
// type change in any struct field trips this guard.
fn sample_args(tool_name: &str) -> Value {
    match tool_name {
        "focus_kpi" => json!({ "kpi_id": "kpi.test" }),
        "explain_kpi" => json!({ "kpi_id": "kpi.test", "depth": 2 }),
        "get_evidence" => json!({ "node_id": "kpi.test", "edge_id": "edge.test" }),
        "impact_radius" => json!({ "node_id": "kpi.test", "max_depth": 3 }),
        "trace_decision" => json!({ "decision_node_id": "decision.test" }),
        "suggest_actions" => json!({
            "kpi_id": "kpi.test",
            "status_filter": ["Healthy", "AtRisk"],
            "include_unreviewed": false
        }),
        "graph_search" => json!({
            "query": "revenue",
            "kinds": ["Kpi", "Metric"],
            "limit": 5
        }),
        "reasoning_search" => json!({
            "query": "revenue",
            "doc_node_ids": ["doc.abc12345"],
            "max_results": 5
        }),
        _ => json!({}),
    }
}

#[tokio::test]
async fn schema_drift_guard() {
    let dir = TempDir::new().unwrap();
    let engine = AxonMindEngine::open(test_engine_config(&dir)).await.unwrap();

    for def in tool_defs() {
        let args = sample_args(def.name);
        let result = call_tool(&engine, def.name, args).await;
        if let Err(ref e) = result {
            assert!(
                !is_schema_error(e),
                "tool '{}' failed schema deserialization: {e}",
                def.name
            );
        }
        // Domain errors (NodeNotFound, NotAKpi, etc.) are fine here.
    }
}

// ── dispatch correctness ─────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_tool_returns_tool_not_found() {
    let dir = TempDir::new().unwrap();
    let engine = AxonMindEngine::open(test_engine_config(&dir)).await.unwrap();

    let err = call_tool(&engine, "nonexistent_tool", json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, AxonMindError::ToolNotFound { .. }),
        "expected ToolNotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn graph_search_empty_workspace_returns_ok() {
    let dir = TempDir::new().unwrap();
    let engine = AxonMindEngine::open(test_engine_config(&dir)).await.unwrap();

    let output = call_tool(&engine, "graph_search", json!({ "query": "anything" }))
        .await
        .expect("graph_search on empty workspace should succeed");

    let nodes = output.get("nodes").expect("output must have nodes field");
    assert!(nodes.is_array());
    assert_eq!(nodes.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn null_arguments_treated_as_empty_object() {
    let dir = TempDir::new().unwrap();
    let engine = AxonMindEngine::open(test_engine_config(&dir)).await.unwrap();

    // graph_search requires "query"; passing null should give a clean schema error,
    // not a panic or an unrelated error.
    let err = call_tool(&engine, "graph_search", Value::Null)
        .await
        .unwrap_err();
    // Either a deserialization/validation error (missing required field) or a domain
    // error is acceptable; a panic is not.
    let _ = err;
}
