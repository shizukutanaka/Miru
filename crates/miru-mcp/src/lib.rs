//! MCP server library.

pub mod local_bridge;
pub mod parent_check;
pub mod protocol;
pub mod rate_limit;
pub mod remote_bridge;
pub mod replay;
pub mod server;
pub mod tools;

use anyhow::Result;
use protocol::{
    errors, InitializeResult, JsonRpcRequest, JsonRpcResponse, ServerCapabilities, ServerInfo,
    ToolCallParams, ToolListResult, ToolsCapability, PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use server::McpServer;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, info, warn};

/// Run the MCP server over stdio (one JSON message per line).
pub async fn run_stdio(server: Arc<McpServer>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    info!("MCP server ready (stdio transport)");

    while let Ok(Some(line)) = reader.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(req) => handle_request(&server, req).await,
            Err(e) => {
                error!("parse error: {} (line: {})", e, line);
                Some(JsonRpcResponse::err(
                    Value::Null,
                    errors::PARSE_ERROR,
                    format!("parse error: {e}"),
                ))
            }
        };

        if let Some(resp) = response {
            let line = serde_json::to_string(&resp)?;
            stdout.write_all(line.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    info!("MCP server: stdin closed, exiting");
    Ok(())
}

async fn handle_request(server: &Arc<McpServer>, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
    let id = req.id.clone().unwrap_or(Value::Null);
    let is_notification = req.id.is_none();

    debug!("MCP <- {}", req.method);

    let result: anyhow::Result<Value> = match req.method.as_str() {
        "initialize" => serde_json::to_value(InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: false,
                },
            },
            server_info: ServerInfo {
                name: "miru-mcp",
                version: env!("CARGO_PKG_VERSION"),
            },
        })
        .map_err(anyhow::Error::from),

        "initialized" | "notifications/initialized" => {
            // Notification — no response
            return None;
        }

        "tools/list" => serde_json::to_value(ToolListResult {
            tools: tools::definitions(),
        })
        .map_err(anyhow::Error::from),

        "tools/call" => {
            let params: ToolCallParams = match req.params {
                Some(p) => match serde_json::from_value(p) {
                    Ok(v) => v,
                    Err(e) => {
                        return Some(JsonRpcResponse::err(
                            id,
                            errors::INVALID_PARAMS,
                            format!("bad params: {e}"),
                        ));
                    }
                },
                None => {
                    return Some(JsonRpcResponse::err(
                        id,
                        errors::INVALID_PARAMS,
                        "missing params",
                    ))
                }
            };
            let result = server.handle_tool_call(params).await;
            serde_json::to_value(result).map_err(anyhow::Error::from)
        }

        "ping" => Ok(json!({})),

        _ => {
            warn!("unknown MCP method: {}", req.method);
            return Some(JsonRpcResponse::err(
                id,
                errors::METHOD_NOT_FOUND,
                format!("method not found: {}", req.method),
            ));
        }
    };

    if is_notification {
        return None;
    }

    Some(match result {
        Ok(r) => JsonRpcResponse::ok(id, r),
        Err(e) => JsonRpcResponse::err(id, errors::INTERNAL_ERROR, e.to_string()),
    })
}
