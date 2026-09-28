//! Bounded MCP message decoding.

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde_json::Value;

use crate::JsonRpcMessage;

/// Resource limits enforced before a message enters the request pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolLimits {
    /// Maximum UTF-8 JSON payload size.
    pub max_message_bytes: usize,
    /// Maximum nested array/object depth.
    pub max_json_depth: usize,
    /// Maximum cumulative array elements and object members.
    pub max_collection_items: usize,
}

impl Default for ProtocolLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 10 * 1024 * 1024,
            max_json_depth: 32,
            max_collection_items: 20_000,
        }
    }
}

/// Decodes one bounded JSON-RPC/MCP message.
///
/// # Errors
///
/// Returns [`AgentMeshError`] for oversized, malformed, deeply nested, or
/// structurally invalid messages.
pub fn decode_message(
    bytes: &[u8],
    limits: ProtocolLimits,
) -> Result<JsonRpcMessage, AgentMeshError> {
    if bytes.len() > limits.max_message_bytes {
        return Err(limit_error(
            "The MCP message exceeds the configured size limit.",
        ));
    }

    let value: Value = serde_json::from_slice(bytes).map_err(|_| {
        AgentMeshError::new(
            ErrorCode::InvalidMessage,
            "The MCP message is not valid JSON.",
        )
    })?;
    let mut collection_items = 0;
    validate_value(&value, 0, &mut collection_items, limits)?;
    JsonRpcMessage::try_from(value)
}

fn validate_value(
    value: &Value,
    depth: usize,
    collection_items: &mut usize,
    limits: ProtocolLimits,
) -> Result<(), AgentMeshError> {
    if depth > limits.max_json_depth {
        return Err(limit_error(
            "The MCP message exceeds the configured nesting limit.",
        ));
    }

    match value {
        Value::Array(values) => {
            add_items(collection_items, values.len(), limits)?;
            for value in values {
                validate_value(value, depth + 1, collection_items, limits)?;
            }
        }
        Value::Object(values) => {
            add_items(collection_items, values.len(), limits)?;
            for value in values.values() {
                validate_value(value, depth + 1, collection_items, limits)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn add_items(
    total: &mut usize,
    additional: usize,
    limits: ProtocolLimits,
) -> Result<(), AgentMeshError> {
    *total = total
        .checked_add(additional)
        .ok_or_else(|| limit_error("The MCP message exceeds the configured collection limit."))?;
    if *total > limits.max_collection_items {
        return Err(limit_error(
            "The MCP message exceeds the configured collection limit.",
        ));
    }
    Ok(())
}

fn limit_error(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::PayloadTooLarge, message)
}
