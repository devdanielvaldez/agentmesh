//! Protocol conformance and defensive-decoding tests.

use std::collections::BTreeMap;

use agentmesh_error::ErrorCode;
use agentmesh_protocol::{
    CapabilitySet, DiscoverResult, DiscoverResultType, Implementation, JsonRpcErrorObject,
    JsonRpcMessage, JsonRpcResponse, McpMethod, McpName, ProtocolEra, ProtocolLimits,
    ProtocolVersion, RequestId, RequestMeta, ResultMeta, SupportedVersions, decode_message,
    negotiate_version,
};
use serde_json::json;

#[test]
fn decodes_modern_request_and_required_metadata() {
    let message = br#"{
        "jsonrpc":"2.0",
        "id":"call-1",
        "method":"tools/list",
        "params":{"_meta":{
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientCapabilities":{"roots":{}},
            "io.modelcontextprotocol/clientInfo":{"name":"test-client","version":"1.0.0"}
        }}
    }"#;

    let decoded = decode_message(message, ProtocolLimits::default()).expect("valid request");
    let JsonRpcMessage::Request(request) = decoded else {
        panic!("expected request");
    };

    assert_eq!(request.method, McpMethod::ToolsList);
    let metadata = request.request_meta().expect("valid metadata");
    assert_eq!(metadata.protocol_version.as_str(), "2026-07-28");
    assert!(metadata.client_capabilities.get("roots").is_some());
    assert_eq!(
        metadata.client_info.expect("client info").name,
        "test-client"
    );
}

#[test]
fn preserves_unknown_methods_and_capabilities() {
    let message = br#"{
        "jsonrpc":"2.0",
        "method":"com.example/widgets/changed",
        "params":{"enabled":true}
    }"#;
    let decoded = decode_message(message, ProtocolLimits::default()).expect("valid notification");
    let JsonRpcMessage::Notification(notification) = decoded else {
        panic!("expected notification");
    };
    assert_eq!(
        notification.method,
        McpMethod::Other("com.example/widgets/changed".into())
    );

    let capabilities: CapabilitySet = serde_json::from_value(json!({
        "tools": {"listChanged": true},
        "com.example/widgets": {"version": 1}
    }))
    .expect("capabilities");
    assert!(capabilities.get("com.example/widgets").is_some());
}

#[test]
fn rejects_conflicting_json_rpc_shapes_and_versions() {
    let conflicting = br#"{
        "jsonrpc":"2.0","id":1,"result":{},
        "error":{"code":-32603,"message":"bad"}
    }"#;
    let wrong_version = br#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#;

    assert_eq!(
        decode_message(conflicting, ProtocolLimits::default())
            .expect_err("conflicting response")
            .code(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        decode_message(wrong_version, ProtocolLimits::default())
            .expect_err("wrong version")
            .code(),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn enforces_size_depth_and_collection_limits() {
    let message = br#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"a":{"b":1}}}"#;

    let size_error = decode_message(
        message,
        ProtocolLimits {
            max_message_bytes: 8,
            ..ProtocolLimits::default()
        },
    )
    .expect_err("size limit");
    assert_eq!(size_error.code(), ErrorCode::PayloadTooLarge);

    let depth_error = decode_message(
        message,
        ProtocolLimits {
            max_json_depth: 2,
            ..ProtocolLimits::default()
        },
    )
    .expect_err("depth limit");
    assert_eq!(depth_error.code(), ErrorCode::PayloadTooLarge);

    let collection_error = decode_message(
        message,
        ProtocolLimits {
            max_collection_items: 2,
            ..ProtocolLimits::default()
        },
    )
    .expect_err("collection limit");
    assert_eq!(collection_error.code(), ErrorCode::PayloadTooLarge);
}

#[test]
fn negotiates_newest_shared_version_across_eras() {
    let modern = ProtocolVersion::parse("2026-07-28").expect("modern version");
    let legacy = ProtocolVersion::parse("2025-11-25").expect("legacy version");
    assert_eq!(modern.era(), ProtocolEra::Modern);
    assert_eq!(legacy.era(), ProtocolEra::Legacy);

    let client = SupportedVersions::new([modern.clone(), legacy.clone()]).expect("client versions");
    let server = SupportedVersions::new([
        ProtocolVersion::parse("2025-06-18").expect("older version"),
        legacy.clone(),
    ])
    .expect("server versions");

    assert_eq!(
        negotiate_version(&client, &server).expect("shared version"),
        legacy
    );
}

#[test]
fn rejects_invalid_dates_and_missing_version_overlap() {
    assert!(ProtocolVersion::parse("2026-02-29").is_err());
    assert!(ProtocolVersion::parse("v2026-07-28").is_err());

    let client = SupportedVersions::latest_only();
    let server =
        SupportedVersions::new([ProtocolVersion::parse("2025-11-25").expect("legacy version")])
            .expect("server versions");
    let error = negotiate_version(&client, &server).expect_err("no overlap");
    assert_eq!(error.code(), ErrorCode::UnsupportedProtocolVersion);
}

#[test]
fn validates_names_without_rejecting_spec_valid_empty_names() {
    assert!(McpName::parse("").is_ok());
    assert!(McpName::parse("github.pull_request-create_2").is_ok());
    assert!(McpName::parse("-invalid").is_err());
    assert!(McpName::parse("invalid/name").is_err());
}

#[test]
fn serializes_response_and_lifecycle_types_with_wire_names() {
    let response = JsonRpcResponse::error(
        RequestId::Integer(7),
        JsonRpcErrorObject::new(-32001, "Authentication required.", None),
    );
    let response = serde_json::to_value(response).expect("serialize response");
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["error"]["code"], -32001);

    let result = DiscoverResult {
        result_type: DiscoverResultType::Complete,
        supported_versions: vec![ProtocolVersion::latest()],
        capabilities: CapabilitySet::new(),
        instructions: None,
        metadata: Some(ResultMeta {
            server_info: Some(Implementation::new("agentmesh", "0.1.0")),
            extensions: BTreeMap::new(),
        }),
        extensions: BTreeMap::new(),
    };
    let result = serde_json::to_value(result).expect("serialize discovery");
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["supportedVersions"][0], "2026-07-28");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "agentmesh"
    );
}

#[test]
fn request_metadata_uses_reserved_wire_keys() {
    let metadata = RequestMeta::new(ProtocolVersion::latest(), CapabilitySet::new());
    let value = serde_json::to_value(metadata).expect("serialize metadata");
    assert_eq!(
        value["io.modelcontextprotocol/protocolVersion"],
        "2026-07-28"
    );
    assert!(
        value
            .get("io.modelcontextprotocol/clientCapabilities")
            .is_some()
    );
}
