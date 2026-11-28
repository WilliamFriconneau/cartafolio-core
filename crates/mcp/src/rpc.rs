use crate::texts;
use crate::{schema, Mcp};
use pensine_access::Principal;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Call {
    name: String,
    #[serde(default = "empty_arguments")]
    arguments: Value,
}

fn empty_arguments() -> Value {
    json!({})
}

impl Mcp {
    pub fn handle(&self, principal: &Principal, request: Value) -> Value {
        let Some(object) = request.as_object() else {
            return rpc_error(&Value::Null, -32600, texts::INVALID_REQUEST);
        };
        let id = object.get("id").unwrap_or(&Value::Null);
        if !(id.is_string() || id.is_number() || id.is_null()) {
            return rpc_error(&Value::Null, -32600, texts::INVALID_REQUEST);
        }
        let method = match (
            object.get("jsonrpc").and_then(Value::as_str),
            object.get("method").and_then(Value::as_str),
        ) {
            (Some("2.0"), Some(method)) => method,
            _ => return rpc_error(id, -32600, texts::INVALID_REQUEST),
        };
        if !object.contains_key("id") {
            return Value::Null;
        }
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-03-26",
                "serverInfo": {"name": "pensine", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"tools": {}},
                "instructions": texts::INSTRUCTIONS,
            }),
            "ping" => json!({}),
            "tools/list" => json!({"tools": schema::tools(principal)}),
            "tools/call" => {
                let Ok(call) = serde_json::from_value::<Call>(request["params"].clone()) else {
                    return rpc_error(id, -32602, texts::INVALID_PARAMS);
                };
                match self.call(principal, &call.name, call.arguments) {
                    Ok(result) => content(false, result.to_string()),
                    Err(error) => content(true, error.to_string()),
                }
            }
            _ => return rpc_error(id, -32601, texts::METHOD_NOT_FOUND),
        };
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }
}

fn rpc_error(id: &Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn content(is_error: bool, text: String) -> Value {
    json!({"isError":is_error,"content":[{"type":"text","text":text}]})
}
