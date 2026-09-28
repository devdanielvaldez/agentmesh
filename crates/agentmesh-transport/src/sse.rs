//! Bounded Server-Sent Events encoding for Streamable HTTP responses.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, ProtocolLimits, decode_message};

/// Encodes one MCP message as an SSE `message` event.
///
/// # Errors
///
/// Returns [`AgentMeshError`] when serialization fails or the encoded JSON
/// exceeds the configured message size.
pub fn encode_sse_event(
    message: &JsonRpcMessage,
    limits: ProtocolLimits,
) -> Result<Vec<u8>, AgentMeshError> {
    let json = serde_json::to_vec(message).map_err(|error| {
        AgentMeshError::with_source(
            ErrorCode::Internal,
            "The MCP message could not be encoded.",
            error,
        )
    })?;
    if json.len() > limits.max_message_bytes {
        return Err(AgentMeshError::new(
            ErrorCode::PayloadTooLarge,
            "The MCP SSE event exceeds the configured size limit.",
        ));
    }

    let mut event = Vec::with_capacity(json.len() + 23);
    event.extend_from_slice(b"event: message\ndata: ");
    event.extend_from_slice(&json);
    event.extend_from_slice(b"\n\n");
    Ok(event)
}

/// Decodes one complete SSE event containing an MCP message.
///
/// Comment, `id`, and `retry` fields are ignored. Multiple `data` lines are
/// joined with a newline according to the SSE processing model.
///
/// # Errors
///
/// Returns [`AgentMeshError`] for unsupported event types, missing data,
/// excessive size, or an invalid MCP message.
pub fn decode_sse_event(
    event: &[u8],
    limits: ProtocolLimits,
) -> Result<JsonRpcMessage, AgentMeshError> {
    if event.len() > limits.max_message_bytes.saturating_add(8 * 1024) {
        return Err(AgentMeshError::new(
            ErrorCode::PayloadTooLarge,
            "The MCP SSE event exceeds the configured size limit.",
        ));
    }
    let event = std::str::from_utf8(event).map_err(|_| {
        AgentMeshError::new(
            ErrorCode::InvalidMessage,
            "The MCP SSE event is not valid UTF-8.",
        )
    })?;

    let mut event_type = None;
    let mut data_lines = Vec::new();
    for raw_line in event.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').map_or((line, ""), |(field, value)| {
            (field, value.strip_prefix(' ').unwrap_or(value))
        });
        match field {
            "event" => event_type = Some(value),
            "data" => data_lines.push(value),
            _ => {}
        }
    }

    if !matches!(event_type, None | Some("message")) {
        return Err(AgentMeshError::new(
            ErrorCode::InvalidMessage,
            "The SSE event type is not an MCP message.",
        ));
    }
    if data_lines.is_empty() {
        return Err(AgentMeshError::new(
            ErrorCode::InvalidMessage,
            "The MCP SSE event does not contain data.",
        ));
    }

    let data = data_lines.join("\n");
    decode_message(data.as_bytes(), limits)
}
