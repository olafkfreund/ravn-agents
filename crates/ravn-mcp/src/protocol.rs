//! The wire types for JSON-RPC 2.0 and the slice of the Model Context Protocol
//! (MCP) this server speaks.
//!
//! We implement the protocol directly rather than depend on an MCP SDK: the
//! surface we need (`initialize`, `tools/list`, `tools/call`, `ping`) is small,
//! and the rest of this workspace is deliberately dependency-light. Keeping the
//! framing here also lets the [`crate::server`] loop stay trivially testable.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The MCP protocol revision this server implements. Sent back in the
/// `initialize` result; clients negotiate against it.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC request id: either a number or a string (per the spec). Notifications
/// have no id. We keep it opaque and echo it back verbatim on the response.
pub type RequestId = Value;

/// An inbound JSON-RPC 2.0 message. A request has an `id`; a notification does
/// not. We accept both through one type and branch on `id` being present.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Absent for notifications (which receive no response).
    #[serde(default)]
    pub id: Option<RequestId>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC 2.0 response. Exactly one of `result` / `error` is set.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// A successful response carrying `result`.
    pub fn ok(id: RequestId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failed response carrying a JSON-RPC error object.
    pub fn err(id: RequestId, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// `-32700`: the server received invalid JSON.
    pub fn parse_error(detail: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: detail.into(),
            data: None,
        }
    }

    /// `-32601`: the method does not exist / is not available.
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
            data: None,
        }
    }

    /// `-32602`: invalid method parameters.
    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: detail.into(),
            data: None,
        }
    }

    /// `-32603`: an internal error while handling the request.
    pub fn internal(detail: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: detail.into(),
            data: None,
        }
    }
}

/// The `initialize` result: who we are and what we can do.
#[derive(Debug, Serialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// The capabilities we advertise. We only expose tools (no resources/prompts).
#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

/// Tools capability flags. We do not emit `listChanged` notifications — the tool
/// set is fixed at startup.
#[derive(Debug, Serialize)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged")]
    pub list_changed: bool,
}

/// Identifies this server to the client.
#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// A single tool as advertised in `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: String,
    /// JSON Schema for the tool's arguments.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// Behavioural annotations (MCP `ToolAnnotations`): hints to the client about
    /// whether a tool mutates state. We set `readOnlyHint` truthfully so a client
    /// can surface "this tool changes things" prompts.
    pub annotations: ToolAnnotations,
}

/// MCP tool annotations. We populate the read-only / destructive hints honestly
/// so clients (and humans) can reason about a tool before calling it.
#[derive(Debug, Clone, Serialize)]
pub struct ToolAnnotations {
    pub title: &'static str,
    #[serde(rename = "readOnlyHint")]
    pub read_only_hint: bool,
    #[serde(rename = "destructiveHint")]
    pub destructive_hint: bool,
    #[serde(rename = "idempotentHint")]
    pub idempotent_hint: bool,
    #[serde(rename = "openWorldHint")]
    pub open_world_hint: bool,
}

/// The result of `tools/call`. `content` is a list of content blocks; we always
/// emit a single text block carrying pretty-printed JSON. `is_error` marks a
/// tool-level (as opposed to protocol-level) failure.
#[derive(Debug, Serialize)]
pub struct CallToolResult {
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

impl CallToolResult {
    /// A successful tool result rendering `value` as pretty JSON text.
    pub fn json(value: &Value) -> Self {
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: false,
        }
    }

    /// A tool-level error (the call reached the tool but the tool failed). Per
    /// MCP, this is reported with `isError: true` inside a normal result rather
    /// than a JSON-RPC error, so the model can see and react to it.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(message.into())],
            is_error: true,
        }
    }
}

/// A content block in a tool result. We only emit text blocks.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentBlock {
    Text { text: String },
}

impl ContentBlock {
    fn text(text: String) -> Self {
        ContentBlock::Text { text }
    }
}

/// Arguments to `tools/call`.
#[derive(Debug, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}
