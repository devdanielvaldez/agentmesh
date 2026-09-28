//! Streamable HTTP header and response-mode rules.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{
    JsonRpcMessage, ProtocolEra, ProtocolVersion, RequestMeta, SupportedVersions,
};
use http::{HeaderMap, header};
use serde_json::Value;

/// HTTP header carrying the MCP protocol version.
pub const MCP_PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";
const LEGACY_HTTP_FALLBACK_VERSION: &str = "2025-03-26";

/// Representation selected for a Streamable HTTP response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseMode {
    /// One JSON-RPC object with `application/json`.
    Json,
    /// A request-scoped `text/event-stream` response.
    ServerSentEvents,
}

/// Resolves and validates the protocol revision for an HTTP MCP request.
///
/// Modern requests must include the same revision in both the
/// `MCP-Protocol-Version` header and request `_meta`. Legacy requests without
/// either value fall back to `2025-03-26` for backward compatibility.
///
/// # Errors
///
/// Returns [`AgentMeshError`] for duplicate, malformed, mismatched, missing,
/// or unsupported protocol versions, and for response objects received where
/// an HTTP request or notification is required.
pub fn resolve_http_protocol_version(
    headers: &HeaderMap,
    message: &JsonRpcMessage,
    supported: &SupportedVersions,
) -> Result<ProtocolVersion, AgentMeshError> {
    let header_version = parse_header_version(headers)?;
    let metadata_version = parse_metadata_version(message)?;

    if let (Some(header), Some(metadata)) = (&header_version, &metadata_version) {
        if header != metadata {
            return Err(invalid_request(
                "The MCP protocol header and request metadata do not match.",
            ));
        }
    }

    let selected = match (&header_version, &metadata_version) {
        (Some(version), _) | (_, Some(version)) => version.clone(),
        (None, None) => ProtocolVersion::parse(LEGACY_HTTP_FALLBACK_VERSION)?,
    };

    if selected.era() == ProtocolEra::Modern
        && (header_version.is_none() || metadata_version.is_none())
    {
        return Err(invalid_request(
            "Modern MCP requests require matching protocol versions in the HTTP header and _meta.",
        ));
    }
    if !supported.iter().any(|version| version == &selected) {
        return Err(AgentMeshError::new(
            ErrorCode::UnsupportedProtocolVersion,
            "The requested MCP protocol version is not supported.",
        ));
    }

    Ok(selected)
}

/// Selects JSON or SSE from the HTTP `Accept` header.
///
/// Missing and wildcard `Accept` headers default to JSON. SSE is selected when
/// explicitly accepted because it can carry streaming and multiple responses.
///
/// # Errors
///
/// Returns [`AgentMeshError`] when neither `application/json` nor
/// `text/event-stream` is acceptable.
pub fn select_response_mode(headers: &HeaderMap) -> Result<ResponseMode, AgentMeshError> {
    let Some(accept) = headers.get(header::ACCEPT) else {
        return Ok(ResponseMode::Json);
    };
    let accept = accept
        .to_str()
        .map_err(|_| invalid_request("The HTTP Accept header is invalid."))?;
    let media_types = accept.split(',').map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    });
    let media_types = media_types.collect::<Vec<_>>();

    if media_types.iter().any(|value| value == "text/event-stream") {
        Ok(ResponseMode::ServerSentEvents)
    } else if media_types
        .iter()
        .any(|value| matches!(value.as_str(), "application/json" | "*/*"))
    {
        Ok(ResponseMode::Json)
    } else {
        Err(invalid_request(
            "The HTTP client does not accept an MCP response media type.",
        ))
    }
}

/// Validates that the HTTP request body is JSON.
///
/// Parameters such as `charset=utf-8` are allowed and compared
/// case-insensitively.
///
/// # Errors
///
/// Returns [`AgentMeshError`] when `Content-Type` is missing, malformed, or is
/// not `application/json`.
pub fn validate_json_content_type(headers: &HeaderMap) -> Result<(), AgentMeshError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .ok_or_else(|| invalid_request("The HTTP Content-Type header is required."))?
        .to_str()
        .map_err(|_| invalid_request("The HTTP Content-Type header is invalid."))?;
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    if media_type.eq_ignore_ascii_case("application/json") {
        Ok(())
    } else {
        Err(invalid_request(
            "MCP Streamable HTTP requests must use application/json.",
        ))
    }
}

fn parse_header_version(headers: &HeaderMap) -> Result<Option<ProtocolVersion>, AgentMeshError> {
    let values = headers
        .get_all(MCP_PROTOCOL_VERSION_HEADER)
        .iter()
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [value] => {
            let value = value
                .to_str()
                .map_err(|_| invalid_request("The MCP protocol version header is invalid."))?;
            ProtocolVersion::parse(value.to_owned()).map(Some)
        }
        _ => Err(invalid_request(
            "The MCP protocol version header must occur exactly once.",
        )),
    }
}

fn parse_metadata_version(
    message: &JsonRpcMessage,
) -> Result<Option<ProtocolVersion>, AgentMeshError> {
    let params = match message {
        JsonRpcMessage::Request(request) => request.params.as_ref(),
        JsonRpcMessage::Notification(notification) => notification.params.as_ref(),
        JsonRpcMessage::Response(_) => {
            return Err(invalid_request(
                "An MCP response cannot be used as an HTTP request body.",
            ));
        }
    };
    let Some(metadata) = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("_meta"))
        .cloned()
    else {
        return Ok(None);
    };
    serde_json::from_value::<RequestMeta>(metadata)
        .map(|metadata| Some(metadata.protocol_version))
        .map_err(|_| invalid_request("The MCP request metadata is invalid."))
}

fn invalid_request(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::InvalidRequest, message)
}
