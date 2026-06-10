//! MCP (Model Context Protocol) wire types.
//!
//! MCP is a JSON-RPC 2.0 protocol (Anthropic spec, 2024-11-05).
//! Communication: line-delimited JSON over stdio (or Streamable HTTP).
//!
//! Protocol overview:
//!   1. Client sends `initialize` with capabilities.
//!   2. Server responds with its capabilities (`tools`, `resources`, `prompts`).
//!   3. Client sends `tools/list` → server lists available tools.
//!   4. Client sends `tools/call` for each tool invocation.
//!   5. Server responds with the tool's result (text + optional binary content).
//!
//! Reference: https://modelcontextprotocol.io/specification

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: &str = "2024-11-05";

// ─── JSON-RPC envelope ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0", id, result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// ─── MCP types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema for tool input.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolListResult {
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text { text: String },
    /// Base64-encoded image with MIME type.
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: &'static str,
    },
}

// ─── Standard JSON-RPC error codes ────────────────────────────────────────────

pub mod errors {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    /// MCP-specific: tool refused by capability gate.
    pub const TOOL_DENIED: i32 = -32001;
    /// MCP-specific: agent token expired or invalid.
    pub const TOKEN_INVALID: i32 = -32002;
}
