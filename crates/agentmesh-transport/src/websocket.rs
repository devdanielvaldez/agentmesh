//! Bounded MCP text-frame helpers for WebSocket adapters.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, ProtocolLimits, decode_message};

/// Encodes one message into a UTF-8 WebSocket text payload.
///
/// # Errors
///
/// Returns [`AgentMeshError`] for encoding failure or an oversized frame.
pub fn encode_websocket_frame(
    message: &JsonRpcMessage,
    limits: ProtocolLimits,
) -> Result<String, AgentMeshError> {
    let bytes = serde_json::to_vec(message).map_err(AgentMeshError::internal)?;
    if bytes.len() > limits.max_message_bytes {
        return Err(AgentMeshError::new(
            ErrorCode::PayloadTooLarge,
            "The WebSocket MCP frame exceeds its size limit.",
        ));
    }
    String::from_utf8(bytes).map_err(AgentMeshError::internal)
}
/// Decodes one complete WebSocket text payload.
///
/// # Errors
///
/// Returns [`AgentMeshError`] when the frame is not a valid bounded MCP message.
pub fn decode_websocket_frame(
    frame: &str,
    limits: ProtocolLimits,
) -> Result<JsonRpcMessage, AgentMeshError> {
    decode_message(frame.as_bytes(), limits)
}
