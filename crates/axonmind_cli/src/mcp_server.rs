use axonmind_engine::AxonMindEngine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn serve(engine: AxonMindEngine) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut out = tokio::io::BufWriter::new(stdout);

    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let frame = error_frame(Value::Null, -32700, &format!("parse error: {e}"));
                write_frame(&mut out, &frame).await?;
                continue;
            }
        };

        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (no id) — process but do not respond.
        if msg.get("id").is_none() {
            continue;
        }

        let response = match method {
            "initialize" => {
                let protocol_version = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("2025-06-18")
                    .to_owned();
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
            "" => error_frame(id, -32600, "missing method"),
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

async fn write_frame(
    out: &mut tokio::io::BufWriter<tokio::io::Stdout>,
    frame: &Value,
) -> anyhow::Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    out.write_all(&bytes).await?;
    out.flush().await?;
    Ok(())
}
