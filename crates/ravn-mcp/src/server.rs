//! The MCP request handler and the stdio transport loop.
//!
//! [`Handler`] is transport-agnostic and pure-ish (its only side effects are
//! HTTP calls to the control plane), so the protocol behaviour — handshake,
//! `tools/list`, `tools/call`, errors — is unit-tested directly against it. The
//! [`serve_stdio`] loop wires it to newline-delimited JSON-RPC on stdin/stdout,
//! the framing every MCP stdio client (Claude Code included) expects.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::client::ControlPlaneClient;
use crate::protocol::{
    CallToolParams, InitializeResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse,
    ServerCapabilities, ServerInfo, ToolsCapability, PROTOCOL_VERSION,
};
use crate::tools;

const SERVER_NAME: &str = "ravn-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Handles decoded JSON-RPC requests against the control plane.
pub struct Handler {
    client: ControlPlaneClient,
    allow_mutations: bool,
}

impl Handler {
    /// Build a handler. `allow_mutations` decides whether the gated tools are
    /// advertised *and* callable.
    pub fn new(client: ControlPlaneClient, allow_mutations: bool) -> Self {
        Self {
            client,
            allow_mutations,
        }
    }

    /// Handle one request, returning a response — or `None` for notifications
    /// (which by JSON-RPC rule get no reply).
    pub async fn handle(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        // Notifications carry no id and expect no response. The one we care
        // about is `notifications/initialized`; anything else we ignore too.
        let id = req.id.clone()?;

        let response = match req.method.as_str() {
            "initialize" => JsonRpcResponse::ok(id, self.initialize()),
            "ping" => JsonRpcResponse::ok(id, json!({})),
            "tools/list" => JsonRpcResponse::ok(id, self.tools_list()),
            "tools/call" => match self.tools_call(req.params).await {
                Ok(value) => JsonRpcResponse::ok(id, value),
                Err(error) => JsonRpcResponse::err(id, error),
            },
            other => JsonRpcResponse::err(id, JsonRpcError::method_not_found(other)),
        };
        Some(response)
    }

    /// The `initialize` result, advertising our protocol version, tool
    /// capability, identity, and a one-line usage instruction.
    fn initialize(&self) -> Value {
        let mode = if self.allow_mutations {
            "Mutations are ENABLED (approve/reject remediation are available and require an admin token)."
        } else {
            "Read-only mode: mutating tools are not exposed."
        };
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: false,
                },
            },
            server_info: ServerInfo {
                name: SERVER_NAME,
                version: SERVER_VERSION,
            },
            instructions: Some(format!(
                "Ravn control-plane bridge. Use the read tools to inspect agents, events, \
                 topology, and remediation proposals. {mode}"
            )),
        };
        serde_json::to_value(result).expect("InitializeResult serializes")
    }

    /// The `tools/list` result: the catalog for the current mode.
    fn tools_list(&self) -> Value {
        json!({ "tools": tools::tool_catalog(self.allow_mutations) })
    }

    /// Handle `tools/call`: validate params, dispatch, wrap the tool result.
    async fn tools_call(&self, params: Value) -> Result<Value, JsonRpcError> {
        let params: CallToolParams = serde_json::from_value(params)
            .map_err(|e| JsonRpcError::invalid_params(format!("invalid tools/call params: {e}")))?;
        let result = tools::dispatch(
            &self.client,
            self.allow_mutations,
            &params.name,
            &params.arguments,
        )
        .await;
        serde_json::to_value(result)
            .map_err(|e| JsonRpcError::internal(format!("serializing tool result: {e}")))
    }
}

/// Run the stdio JSON-RPC loop until EOF on stdin. Each line is one JSON-RPC
/// message; each response is written as one line to stdout. Malformed lines get
/// a parse-error response (with a null id, per spec) and the loop continues.
pub async fn serve_stdio(handler: Handler) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(req) => handler.handle(req).await,
            Err(error) => Some(JsonRpcResponse::err(
                Value::Null,
                JsonRpcError::parse_error(format!("invalid JSON-RPC message: {error}")),
            )),
        };
        if let Some(response) = response {
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            stdout.write_all(&bytes).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler(allow_mutations: bool) -> Handler {
        let client = ControlPlaneClient::new("http://127.0.0.1:1", None).unwrap();
        Handler::new(client, allow_mutations)
    }

    fn req(id: i64, method: &str, params: Value) -> JsonRpcRequest {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn initialize_advertises_protocol_and_identity() {
        let resp = handler(false)
            .handle(req(1, "initialize", json!({})))
            .await
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    }

    #[tokio::test]
    async fn tools_list_is_read_only_by_default() {
        let resp = handler(false)
            .handle(req(2, "tools/list", json!({})))
            .await
            .unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        let names: Vec<String> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"list_agents".to_string()));
        assert!(!names.contains(&"approve_remediation".to_string()));
        assert_eq!(tools.len(), 5);
    }

    #[tokio::test]
    async fn tools_list_exposes_mutations_when_enabled() {
        let resp = handler(true)
            .handle(req(3, "tools/list", json!({})))
            .await
            .unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 7);
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let resp = handler(false)
            .handle(req(4, "frobnicate", json!({})))
            .await
            .unwrap();
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn notification_gets_no_response() {
        let note: JsonRpcRequest = serde_json::from_value(json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }))
        .unwrap();
        assert!(handler(false).handle(note).await.is_none());
    }

    #[tokio::test]
    async fn calling_gated_tool_in_read_only_mode_is_a_tool_error() {
        // No network call happens: dispatch refuses before reaching the client.
        let resp = handler(false)
            .handle(req(
                5,
                "tools/call",
                json!({ "name": "approve_remediation", "arguments": { "id": "x" } }),
            ))
            .await
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("read-only"), "got: {text}");
    }

    #[tokio::test]
    async fn calling_unknown_tool_is_a_tool_error() {
        let resp = handler(false)
            .handle(req(
                6,
                "tools/call",
                json!({ "name": "nope", "arguments": {} }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.result.unwrap()["isError"], true);
    }

    #[tokio::test]
    async fn tools_call_with_bad_params_is_invalid_params() {
        // `name` missing → params fail to deserialize.
        let resp = handler(false)
            .handle(req(7, "tools/call", json!({ "arguments": {} })))
            .await
            .unwrap();
        assert_eq!(resp.error.unwrap().code, -32602);
    }
}
