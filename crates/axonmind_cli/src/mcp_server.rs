use axonmind_engine::AxonMindEngine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter};

const PREFERRED_PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2024-11-05"];

pub async fn serve(engine: AxonMindEngine) -> anyhow::Result<()> {
    serve_io(engine, tokio::io::stdin(), tokio::io::stdout()).await
}

async fn serve_io<R, W>(engine: AxonMindEngine, reader: R, writer: W) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    let mut out = BufWriter::new(writer);
    let mut initialized = false;

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        // 1. Parse JSON
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let frame = error_frame(Value::Null, -32700, &format!("parse error: {e}"));
                write_frame(&mut out, &frame).await?;
                continue;
            }
        };

        // 2. Validate JSON-RPC 2.0 envelope
        if msg.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            let frame = error_frame(id, -32600, "invalid request: jsonrpc must be \"2.0\"");
            write_frame(&mut out, &frame).await?;
            continue;
        }

        let method = match msg.get("method").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let frame = error_frame(id, -32600, "invalid request: method must be a string");
                write_frame(&mut out, &frame).await?;
                continue;
            }
        };

        if let Some(params) = msg.get("params") {
            if !params.is_object() && !params.is_array() && !params.is_null() {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let frame = error_frame(
                    id,
                    -32600,
                    "invalid request: params must be object or array",
                );
                write_frame(&mut out, &frame).await?;
                continue;
            }
        }

        // 3. Notifications (no id field) — process but do not respond
        if msg.get("id").is_none() {
            continue;
        }

        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // 4. Dispatch
        let response = match method {
            "initialize" => {
                let client_version = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let protocol_version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&client_version) {
                    client_version.to_owned()
                } else {
                    PREFERRED_PROTOCOL_VERSION.to_owned()
                };
                initialized = true;
                result_frame(
                    id,
                    json!({
                        "protocolVersion": protocol_version,
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "axonmind",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }),
                )
            }
            "ping" => result_frame(id, json!({})),
            "tools/list" | "tools/call" if !initialized => {
                error_frame(id, -32600, "server not initialized: send initialize first")
            }
            "tools/list" => {
                let tools: Vec<Value> = axonmind_engine::mcp::list_tools()
                    .into_iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.input_schema
                        })
                    })
                    .collect();
                result_frame(id, json!({ "tools": tools }))
            }
            "tools/call" => {
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if tool_name.is_empty() {
                    error_frame(id, -32602, "tools/call requires params.name")
                } else {
                    let arguments = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or(Value::Object(Default::default()));
                    match axonmind_engine::mcp::call_tool(&engine, tool_name, arguments).await {
                        Ok(output) => {
                            let text = serde_json::to_string_pretty(&output)
                                .unwrap_or_else(|e| e.to_string());
                            result_frame(
                                id,
                                json!({
                                    "content": [{ "type": "text", "text": text }],
                                    "isError": false
                                }),
                            )
                        }
                        // Unknown tool → JSON-RPC invalid params, not isError:true
                        Err(axonmind_core::AxonMindError::ToolNotFound { name }) => {
                            error_frame(id, -32602, &format!("unknown tool: {name}"))
                        }
                        Err(e) => result_frame(
                            id,
                            json!({
                                "content": [{ "type": "text", "text": e.to_string() }],
                                "isError": true
                            }),
                        ),
                    }
                }
            }
            _ => error_frame(id, -32601, &format!("method not found: {method}")),
        };

        write_frame(&mut out, &response).await?;
    }

    Ok(())
}

fn result_frame(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_frame(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

async fn write_frame<W: AsyncWrite + Unpin>(
    out: &mut BufWriter<W>,
    frame: &Value,
) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    out.write_all(&bytes).await?;
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonmind_engine::{AxonMindEngine, config::EngineConfig};
    use tempfile::TempDir;

    async fn run_frames(input: &str) -> Vec<Value> {
        let dir = TempDir::new().unwrap();
        let engine =
            AxonMindEngine::open(EngineConfig::from_workspace_dir(dir.path().to_path_buf()))
                .await
                .unwrap();
        let mut output: Vec<u8> = Vec::new();
        serve_io(engine, input.as_bytes(), &mut output)
            .await
            .unwrap();
        output
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_slice(l).expect("response must be valid JSON"))
            .collect()
    }

    const INIT: &str = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{}}}\n";

    #[tokio::test]
    async fn initialize_returns_server_info_and_preferred_version() {
        let frames = run_frames(INIT).await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["id"], 1);
        assert_eq!(frames[0]["result"]["serverInfo"]["name"], "axonmind");
        assert_eq!(frames[0]["result"]["protocolVersion"], "2025-06-18");
    }

    #[tokio::test]
    async fn unsupported_protocol_version_echoes_preferred() {
        let frames = run_frames(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"1900-01-01\",\"capabilities\":{}}}\n",
        )
        .await;
        assert_eq!(
            frames[0]["result"]["protocolVersion"],
            PREFERRED_PROTOCOL_VERSION
        );
    }

    #[tokio::test]
    async fn initialize_then_tools_list_returns_eight_tools() {
        let input = format!("{INIT}{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}}\n");
        let frames = run_frames(&input).await;
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1]["id"], 2);
        assert_eq!(frames[1]["result"]["tools"].as_array().unwrap().len(), 8);
    }

    #[tokio::test]
    async fn tools_list_before_initialize_returns_rpc_error() {
        let frames = run_frames("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n").await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["error"]["code"], -32600);
        assert!(frames[0].get("result").is_none());
    }

    #[tokio::test]
    async fn tools_call_before_initialize_returns_rpc_error() {
        let frames = run_frames("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"graph_search\",\"arguments\":{\"query\":\"x\"}}}\n").await;
        assert_eq!(frames[0]["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn unknown_tool_returns_invalid_params_not_is_error() {
        let input = format!(
            "{INIT}{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{{\"name\":\"no_such_tool\",\"arguments\":{{}}}}}}\n"
        );
        let frames = run_frames(&input).await;
        assert_eq!(frames[1]["error"]["code"], -32602);
        assert!(frames[1].get("result").is_none());
    }

    #[tokio::test]
    async fn invalid_jsonrpc_version_returns_invalid_request() {
        let frames = run_frames("{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"ping\"}\n").await;
        assert_eq!(frames[0]["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn missing_method_returns_invalid_request() {
        let frames = run_frames("{\"jsonrpc\":\"2.0\",\"id\":1,\"foo\":\"bar\"}\n").await;
        assert_eq!(frames[0]["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let input =
            format!("{INIT}{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"no_such_method\"}}\n");
        let frames = run_frames(&input).await;
        assert_eq!(frames[1]["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn notification_produces_no_response() {
        // notifications/initialized has no id — must not produce a response frame
        let input =
            format!("{INIT}{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}}\n");
        let frames = run_frames(&input).await;
        assert_eq!(frames.len(), 1, "notification must not produce a response");
    }
}
