//! SSRF, rebinding, payload, integrity, header, and redaction tests.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr},
};

use agentmesh_error::ErrorCode;
use agentmesh_security::{
    EgressPolicy, IntegrityDecision, PayloadGuard, integrity_digest, redact_metadata,
    sanitize_headers, verify_integrity,
};
use serde_json::json;

fn policy() -> EgressPolicy {
    EgressPolicy::new(
        BTreeSet::from(["api.example.com".into(), ".trusted.example".into()]),
        BTreeSet::from([443]),
        false,
    )
    .unwrap()
}

#[test]
fn egress_blocks_local_metadata_credentials_and_rebinding() {
    let public = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
    let pinned = policy()
        .validate_destination("https://api.example.com/mcp", &[public])
        .unwrap();
    assert_eq!(pinned.host(), "api.example.com");
    assert!(pinned.validate_re_resolution(&[public]).is_ok());
    for (url, address) in [
        (
            "https://api.example.com/mcp",
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        ),
        (
            "https://api.example.com/mcp",
            IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)),
        ),
        ("https://user:pass@api.example.com/mcp", public),
        ("https://evil.example/mcp", public),
    ] {
        assert_eq!(
            policy()
                .validate_destination(url, &[address])
                .unwrap_err()
                .code(),
            ErrorCode::PermissionDenied
        );
    }
    assert_eq!(
        pinned
            .validate_re_resolution(&[IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))])
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
}

#[test]
fn payload_and_integrity_guards_are_bounded() {
    let guard = PayloadGuard {
        max_bytes: 64,
        max_depth: 3,
    };
    assert_eq!(
        guard.decode(br#"{"a":{"b":{"c":1}}}"#).unwrap_err().code(),
        ErrorCode::PayloadTooLarge
    );
    let definition = json!({"name":"weather","schema":{"type":"object"}});
    let digest = integrity_digest(&definition).unwrap();
    assert_eq!(
        verify_integrity(&definition, Some(&digest)).unwrap(),
        IntegrityDecision::Trusted
    );
    assert_eq!(
        verify_integrity(&json!({"name":"changed"}), Some(&digest)).unwrap(),
        IntegrityDecision::Quarantine
    );
}

#[test]
fn headers_and_metadata_strip_secrets() {
    let headers = BTreeMap::from([
        ("Authorization".into(), "Bearer secret".into()),
        ("Mcp-Method".into(), "tools/call".into()),
        ("Connection".into(), "keep-alive".into()),
    ]);
    let safe = sanitize_headers(&headers);
    assert_eq!(safe.len(), 1);
    assert_eq!(safe["mcp-method"], "tools/call");
    let metadata = redact_metadata(&BTreeMap::from([
        ("api_token".into(), "secret".into()),
        ("region".into(), "east".into()),
    ]));
    assert_eq!(metadata["api_token"], "[REDACTED]");
    assert_eq!(metadata["region"], "east");
}
