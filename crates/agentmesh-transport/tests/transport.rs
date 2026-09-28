//! Transport framing, negotiation, and resource-bound tests.

use std::collections::BTreeMap;

use agentmesh_error::ErrorCode;
use agentmesh_protocol::{
    CapabilitySet, JsonRpcMessage, JsonRpcRequest, McpMethod, ProtocolLimits, ProtocolVersion,
    RequestId, RequestMeta, SupportedVersions,
};
use agentmesh_transport::{
    MCP_PROTOCOL_VERSION_HEADER, McpTransport, ResponseMode, StdioTransport, TransportKind,
    decode_sse_event, encode_sse_event, resolve_http_protocol_version, select_response_mode,
    validate_json_content_type,
};
use http::{HeaderMap, HeaderValue, header};
use serde_json::json;
use tokio::io::{AsyncWriteExt, split};

fn ping_message() -> JsonRpcMessage {
    JsonRpcMessage::Request(JsonRpcRequest::new(
        RequestId::Integer(1),
        McpMethod::Ping,
        Some(json!({})),
    ))
}

fn modern_message() -> JsonRpcMessage {
    let metadata = RequestMeta {
        protocol_version: ProtocolVersion::latest(),
        client_capabilities: CapabilitySet::new(),
        client_info: None,
        progress_token: None,
        extensions: BTreeMap::new(),
    };
    JsonRpcMessage::Request(JsonRpcRequest::new(
        RequestId::String("modern-1".into()),
        McpMethod::ToolsList,
        Some(json!({"_meta": metadata})),
    ))
}

#[tokio::test]
async fn stdio_round_trip_uses_newline_framing() {
    let (side_a, side_b) = tokio::io::duplex(4096);
    let (read_a, write_a) = split(side_a);
    let (read_b, write_b) = split(side_b);
    let limits = ProtocolLimits::default();
    let mut client = StdioTransport::new(read_a, write_a, limits);
    let mut server = StdioTransport::new(read_b, write_b, limits);
    let message = ping_message();

    client.send(&message).await.expect("send request");
    let received = server
        .receive()
        .await
        .expect("receive request")
        .expect("message before EOF");

    assert_eq!(received, message);
    assert_eq!(client.kind(), TransportKind::Stdio);
}

#[tokio::test]
async fn stdio_rejects_oversized_frames_without_unbounded_buffering() {
    let (side_a, side_b) = tokio::io::duplex(4096);
    let (read_a, write_a) = split(side_a);
    let (read_b, mut write_b) = split(side_b);
    let limits = ProtocolLimits {
        max_message_bytes: 32,
        ..ProtocolLimits::default()
    };
    let mut receiver = StdioTransport::new(read_a, write_a, limits);

    let writer = tokio::spawn(async move {
        write_b.write_all(&[b'x'; 33]).await.expect("write frame");
        write_b.write_all(b"\n").await.expect("write delimiter");
    });
    let error = receiver.receive().await.expect_err("oversized frame");
    writer.await.expect("writer task");

    assert_eq!(error.code(), ErrorCode::PayloadTooLarge);
    assert!(receiver.is_closed());
    drop(read_b);
}

#[test]
fn sse_round_trip_preserves_message() {
    let message = ping_message();
    let limits = ProtocolLimits::default();
    let event = encode_sse_event(&message, limits).expect("encode SSE");

    assert!(event.starts_with(b"event: message\ndata: "));
    assert_eq!(
        decode_sse_event(&event, limits).expect("decode SSE"),
        message
    );
}

#[test]
fn sse_rejects_non_message_events_and_missing_data() {
    let limits = ProtocolLimits::default();
    assert_eq!(
        decode_sse_event(b"event: endpoint\ndata: /mcp\n\n", limits)
            .expect_err("unsupported event")
            .code(),
        ErrorCode::InvalidMessage
    );
    assert!(decode_sse_event(b"event: message\n\n", limits).is_err());
}

#[test]
fn modern_http_version_must_match_header_and_metadata() {
    let message = modern_message();
    let supported = SupportedVersions::latest_only();
    let mut headers = HeaderMap::new();
    headers.insert(
        MCP_PROTOCOL_VERSION_HEADER,
        HeaderValue::from_static("2026-07-28"),
    );

    assert_eq!(
        resolve_http_protocol_version(&headers, &message, &supported)
            .expect("matching modern version")
            .as_str(),
        "2026-07-28"
    );

    headers.insert(
        MCP_PROTOCOL_VERSION_HEADER,
        HeaderValue::from_static("2025-11-25"),
    );
    assert_eq!(
        resolve_http_protocol_version(&headers, &message, &supported)
            .expect_err("mismatched versions")
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn legacy_http_without_version_uses_compatibility_fallback() {
    let supported = SupportedVersions::new([
        ProtocolVersion::parse("2025-03-26").expect("fallback version"),
        ProtocolVersion::latest(),
    ])
    .expect("supported versions");

    assert_eq!(
        resolve_http_protocol_version(&HeaderMap::new(), &ping_message(), &supported)
            .expect("legacy fallback")
            .as_str(),
        "2025-03-26"
    );
}

#[test]
fn content_negotiation_and_content_type_are_explicit() {
    let mut headers = HeaderMap::new();
    assert_eq!(
        select_response_mode(&headers).expect("default response"),
        ResponseMode::Json
    );

    headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    assert_eq!(
        select_response_mode(&headers).expect("SSE accepted"),
        ResponseMode::ServerSentEvents
    );

    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    validate_json_content_type(&headers).expect("JSON content type");
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert!(validate_json_content_type(&headers).is_err());
}
