//! Protocol conformance and defensive-decoding tests.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_error::ErrorCode;
use agentmesh_protocol::{
    ApprovalCheckpoint, AuthorityRequirements, CAPABILITY_PACKAGE_VERSION, CapabilityContract,
    CapabilityDefinition, CapabilityNegotiationRequest, CapabilityPackage, CapabilityRequirement,
    CapabilitySet, CertificationCheck, CertificationLevel, CertificationReport, DiscoverResult,
    DiscoverResultType, EffectKind, EvidenceClaim, EvidenceType, ExecutionEvidence,
    ExecutionReceipt, ExecutionStatus, Idempotency, Implementation, ImplementationBinding,
    ImplementationKind, JsonRpcErrorObject, JsonRpcMessage, JsonRpcResponse, McpMethod, McpName,
    ProtocolEra, ProtocolLimits, ProtocolVersion, RecoveryAction, RecoveryContext,
    RecoveryStrategy, RequestId, RequestMeta, ResultMeta, SupportedVersions, decide_recovery,
    decode_message, discover_capabilities, evaluate_certification, negotiate_capability_offer,
    negotiate_version, resolve_capability_plan, select_implementation, verify_execution_receipt,
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

fn refund_package() -> CapabilityPackage {
    CapabilityPackage {
        schema_version: CAPABILITY_PACKAGE_VERSION.into(),
        capability: CapabilityDefinition {
            id: "billing.refund".into(),
            version: "1.0.0".into(),
            intent: "Refund an eligible customer payment".into(),
        },
        contract: CapabilityContract {
            requires: vec![CapabilityRequirement {
                id: "customer.lookup".into(),
                version: Some("1.0.0".into()),
            }],
            inputs: json!({"type":"object", "required":["payment_id"]}),
            outputs: json!({"type":"object", "required":["refund_id"]}),
            success_evidence: vec![EvidenceClaim {
                id: "payment_refunded".into(),
                assertion: "payment.status == refunded".into(),
                accepted_types: vec![EvidenceType::ProviderReceipt, EvidenceType::StateAssertion],
            }],
            effects: vec![EffectKind::Financial, EffectKind::ExternalCommunication],
            idempotency: Idempotency::KeyRequired,
            recovery: RecoveryStrategy::Reconcile,
        },
        authority: AuthorityRequirements {
            permissions: vec!["customer.read".into(), "payment.refund".into()],
            approvals: vec![ApprovalCheckpoint {
                before: "execute_refund".into(),
                display: vec!["amount".into(), "customer".into()],
                required_role: Some("finance_approver".into()),
            }],
            constraints: BTreeMap::new(),
        },
        implementations: vec![ImplementationBinding {
            id: "stripe-api".into(),
            kind: ImplementationKind::Mcp,
            reference: "mcp://stripe/refund".into(),
            configuration: BTreeMap::new(),
        }],
        provenance: None,
        extensions: BTreeMap::new(),
    }
}

#[test]
fn validates_portable_capability_package() {
    let package = refund_package();
    package.validate().expect("valid portable contract");

    let value = serde_json::to_value(package).expect("serialize capability package");
    assert_eq!(value["schemaVersion"], "amcp/0.1");
    assert_eq!(value["contract"]["effects"][0], "financial");
    assert_eq!(value["implementations"][0]["kind"], "mcp");
}

#[test]
fn package_content_digest_detects_contract_tampering() {
    let mut package = refund_package();
    package.provenance = Some(agentmesh_protocol::Provenance {
        publisher: "agentmesh.example".into(),
        package_digest: String::new(),
        source: agentmesh_protocol::ProvenanceSource::Authored,
        signature: None,
    });
    let digest = package.content_digest().expect("canonical digest");
    package.provenance.as_mut().unwrap().package_digest = digest;
    assert!(package.validate().is_ok());
    assert!(package.verify_content_digest());

    package.capability.intent = "Changed after signing".into();
    assert!(!package.verify_content_digest());
}

#[test]
fn certification_level_requires_matching_digest_and_complete_checks() {
    let mut package = refund_package();
    package.provenance = Some(agentmesh_protocol::Provenance {
        publisher: "publisher.example".into(),
        package_digest: String::new(),
        source: agentmesh_protocol::ProvenanceSource::Authored,
        signature: None,
    });
    let digest = package.content_digest().unwrap();
    package.provenance.as_mut().unwrap().package_digest = digest.clone();
    let report = CertificationReport {
        capability_id: package.capability.id.clone(),
        capability_version: package.capability.version.clone(),
        package_digest: digest,
        reviewed_by: Some("reviewer@example.com".into()),
        checks: BTreeMap::from([(
            "contract".into(),
            CertificationCheck {
                passed: true,
                evidence_reference: "test-run-1".into(),
            },
        )]),
    };
    let decision = evaluate_certification(&package, &report);
    assert_eq!(decision.level, CertificationLevel::Reviewed);
    assert!(decision.missing_requirements.contains("policy"));
}

#[test]
fn rejects_ambiguous_or_unsafe_capability_packages() {
    let mut package = refund_package();
    package.capability.id = "Refund".into();
    assert!(package.validate().is_err());

    let mut package = refund_package();
    package.contract.success_evidence.clear();
    assert!(package.validate().is_err());

    let mut package = refund_package();
    package.authority.permissions.push("payment.refund".into());
    assert!(package.validate().is_err());

    let mut package = refund_package();
    package.implementations[0].reference.clear();
    assert!(package.validate().is_err());
}

#[test]
fn verifies_receipts_against_declared_success_evidence() {
    let package = refund_package();
    let mut receipt = ExecutionReceipt {
        execution_id: "exec_123".into(),
        capability_id: "billing.refund".into(),
        capability_version: "1.0.0".into(),
        implementation_id: "stripe-api".into(),
        status: ExecutionStatus::Verified,
        evidence: vec![ExecutionEvidence {
            claim_id: "payment_refunded".into(),
            evidence_type: EvidenceType::ProviderReceipt,
            reference: "re_123".into(),
            digest: None,
        }],
        policy_decisions: vec!["approval.approved".into()],
        package_digest: None,
        extensions: BTreeMap::new(),
    };
    assert!(verify_execution_receipt(&package, &receipt).verified);

    receipt.evidence[0].evidence_type = EvidenceType::HumanAttestation;
    let verification = verify_execution_receipt(&package, &receipt);
    assert!(!verification.verified);
    assert!(verification.missing_claims.contains("payment_refunded"));
}

#[test]
fn resolves_composed_capabilities_in_dependency_first_order() {
    let mut root = refund_package();
    let mut dependency = refund_package();
    dependency.capability.id = "customer.lookup".into();
    dependency.contract.requires.clear();
    root.contract.requires = vec![CapabilityRequirement {
        id: "customer.lookup".into(),
        version: Some("1.0.0".into()),
    }];

    let catalog = [root, dependency];
    let plan =
        resolve_capability_plan("billing.refund", &catalog).expect("resolvable capability graph");
    assert_eq!(plan[0].capability.id, "customer.lookup");
    assert_eq!(plan[1].capability.id, "billing.refund");
}

#[test]
fn discovers_capabilities_by_intent_and_ranks_direct_id_matches_first() {
    let mut refund = refund_package();
    refund.contract.requires.clear();
    let mut lookup = refund_package();
    lookup.capability.id = "customer.lookup".into();
    lookup.capability.intent = "Find a customer record".into();
    lookup.contract.requires.clear();
    let catalog = [lookup, refund];

    let results = discover_capabilities("refund customer payment", &catalog, 10);
    assert_eq!(results[0].package.capability.id, "billing.refund");
    assert!(results[0].matched_terms.contains(&"refund".into()));
    assert!(discover_capabilities("   ", &catalog, 10).is_empty());
}

#[test]
fn selects_only_available_bindings_in_runtime_preference_order() {
    let package = refund_package();
    let available = BTreeSet::from(["stripe-api".to_owned()]);
    let binding = select_implementation(
        &package,
        &available,
        &[
            ImplementationKind::AgentmeshWorkflow,
            ImplementationKind::Mcp,
        ],
    )
    .expect("available MCP binding");
    assert_eq!(binding.id, "stripe-api");

    assert!(select_implementation(&package, &BTreeSet::new(), &[ImplementationKind::Mcp]).is_err());
}

#[test]
fn recovery_reconciles_unknown_state_and_gates_retries_by_idempotency() {
    let mut package = refund_package();
    package.contract.recovery = RecoveryStrategy::Retry;
    let decision = decide_recovery(
        &package,
        ExecutionStatus::Failed,
        RecoveryContext {
            attempts_used: 1,
            maximum_attempts: 3,
            idempotency_key_available: true,
            external_state_unknown: true,
            compensation_available: false,
        },
    );
    assert_eq!(decision.action, RecoveryAction::Reconcile);

    let decision = decide_recovery(
        &package,
        ExecutionStatus::Failed,
        RecoveryContext {
            attempts_used: 1,
            maximum_attempts: 3,
            idempotency_key_available: true,
            external_state_unknown: false,
            compensation_available: false,
        },
    );
    assert_eq!(decision.action, RecoveryAction::Retry);

    let decision = decide_recovery(
        &package,
        ExecutionStatus::Failed,
        RecoveryContext {
            attempts_used: 1,
            maximum_attempts: 3,
            idempotency_key_available: false,
            external_state_unknown: false,
            compensation_available: false,
        },
    );
    assert_eq!(decision.action, RecoveryAction::Reconcile);

    let decision = decide_recovery(
        &package,
        ExecutionStatus::Failed,
        RecoveryContext {
            attempts_used: 1,
            maximum_attempts: 3,
            idempotency_key_available: true,
            external_state_unknown: false,
            compensation_available: false,
        },
    );
    assert_eq!(decision.action, RecoveryAction::Retry);
}

#[test]
fn agents_negotiate_a_concrete_offer_with_effect_and_authority_constraints() {
    let package = refund_package();
    let request = CapabilityNegotiationRequest {
        capability_id: "billing.refund".into(),
        version: Some("1.0.0".into()),
        accepted_effects: BTreeSet::from([
            EffectKind::Financial,
            EffectKind::ExternalCommunication,
        ]),
        granted_permissions: BTreeSet::from(["customer.read".into(), "payment.refund".into()]),
        available_bindings: BTreeSet::from(["stripe-api".into()]),
        preferred_kinds: vec![ImplementationKind::Mcp],
    };
    let offer = negotiate_capability_offer(&request, &[package.clone()]).expect("compatible offer");
    assert_eq!(offer.capability_id, "billing.refund");
    assert_eq!(offer.implementation.id, "stripe-api");

    let mut restricted = request;
    restricted.accepted_effects.clear();
    assert!(negotiate_capability_offer(&restricted, &[package]).is_err());
}
