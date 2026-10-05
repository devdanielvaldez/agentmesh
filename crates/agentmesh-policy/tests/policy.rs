//! Contextual policy and approval workflow integration tests.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_authn::{AuthenticatedPrincipal, AuthenticationEvidence, AuthenticationStrength};
use agentmesh_authz::{Action, Resource};
use agentmesh_error::ErrorCode;
use agentmesh_policy::{
    ApprovalState, ApprovalStore, CapabilityAdmissionPolicy, CapabilityTrustPolicy,
    PackageSignatureVerifier, PolicyBundle, PolicyContext, PolicyEffect, PolicyMatch, PolicyRule,
    Risk, admit_capability, capability_approval_matches, capability_risk, verify_capability_trust,
};
use agentmesh_protocol::{
    CAPABILITY_PACKAGE_VERSION, CapabilityContract, CapabilityDefinition, CapabilityPackage,
    CapabilityRequirement, EffectKind, EvidenceClaim, EvidenceType, ImplementationBinding,
    ImplementationKind, Provenance, ProvenanceSource,
};
use agentmesh_registry::RegistryScope;

fn principal(scope: &RegistryScope) -> AuthenticatedPrincipal {
    AuthenticatedPrincipal {
        id: "alice".into(),
        scope: scope.clone(),
        strength: AuthenticationStrength::Strong,
        asserted_roles: BTreeSet::from(["developer".into()]),
        evidence: AuthenticationEvidence {
            mechanism: "test".into(),
            reference: "fixture".into(),
        },
    }
}

#[test]
fn high_risk_production_calls_require_approval() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let caller = principal(&scope);
    let resource = Resource {
        kind: "tool".into(),
        name: "deploy".into(),
        labels: BTreeMap::new(),
    };
    let bundle = PolicyBundle::compile(
        "acme",
        4,
        vec![PolicyRule {
            id: "approve-prod".into(),
            priority: 100,
            matcher: PolicyMatch {
                actions: BTreeSet::from([Action::ToolCall]),
                roles: BTreeSet::from(["developer".into()]),
                environment: Some("production".into()),
                minimum_risk: Some(Risk::High),
                ..PolicyMatch::default()
            },
            effect: PolicyEffect::RequireApproval,
            reason: "high_risk_production".into(),
            obligations: BTreeSet::from(["audit_required".into()]),
        }],
    )
    .unwrap();
    let metadata = BTreeMap::new();
    let context = PolicyContext {
        principal: &caller,
        scope: &scope,
        action: &Action::ToolCall,
        resource: &resource,
        environment: "production",
        data_classification: "internal",
        risk: Risk::High,
        minute_utc: 600,
        argument_metadata: &metadata,
    };
    let decision = bundle.evaluate(&context);
    assert_eq!(decision.effect, PolicyEffect::RequireApproval);
    assert!(decision.obligations.contains("audit_required"));
    assert_eq!(bundle.simulate(&[context]).unwrap().len(), 1);
}

#[test]
fn default_and_cross_tenant_results_deny() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let caller = principal(&scope);
    let other = RegistryScope::new("other", "prod").unwrap();
    let resource = Resource {
        kind: "tool".into(),
        name: "read".into(),
        labels: BTreeMap::new(),
    };
    let bundle = PolicyBundle::compile("acme", 1, Vec::new()).unwrap();
    let metadata = BTreeMap::new();
    let context = PolicyContext {
        principal: &caller,
        scope: &other,
        action: &Action::ToolCall,
        resource: &resource,
        environment: "prod",
        data_classification: "public",
        risk: Risk::Low,
        minute_utc: 0,
        argument_metadata: &metadata,
    };
    assert_eq!(bundle.evaluate(&context).reason, "tenant_mismatch");
}

#[test]
fn approvals_enforce_expiry_and_separation_of_duties() {
    let store = ApprovalStore::default();
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let pending = store
        .request(scope.clone(), "alice", "sha256:abc", 100, 0)
        .unwrap();
    assert_eq!(
        store
            .resolve(&scope, pending.id, "alice", true, 1)
            .unwrap_err()
            .code(),
        ErrorCode::Conflict
    );
    let approved = store.resolve(&scope, pending.id, "bob", true, 2).unwrap();
    assert_eq!(approved.state, ApprovalState::Approved);
    assert_eq!(approved.resolved_by.as_deref(), Some("bob"));
}

#[test]
fn compiler_rejects_ambiguous_effects() {
    let matcher = PolicyMatch {
        environment: Some("prod".into()),
        ..PolicyMatch::default()
    };
    let rules = vec![
        PolicyRule {
            id: "allow".into(),
            priority: 1,
            matcher: matcher.clone(),
            effect: PolicyEffect::Allow,
            reason: "allow".into(),
            obligations: BTreeSet::new(),
        },
        PolicyRule {
            id: "deny".into(),
            priority: 1,
            matcher,
            effect: PolicyEffect::Deny,
            reason: "deny".into(),
            obligations: BTreeSet::new(),
        },
    ];
    assert_eq!(
        PolicyBundle::compile("acme", 1, rules).unwrap_err().code(),
        ErrorCode::ConfigurationInvalid
    );
}

fn package(effects: Vec<EffectKind>) -> CapabilityPackage {
    let mut package = CapabilityPackage {
        schema_version: CAPABILITY_PACKAGE_VERSION.into(),
        capability: CapabilityDefinition {
            id: "billing.refund".into(),
            version: "1.0.0".into(),
            intent: "Refund a payment".into(),
        },
        contract: CapabilityContract {
            requires: Vec::<CapabilityRequirement>::new(),
            inputs: serde_json::json!({"type":"object"}),
            outputs: serde_json::json!({"type":"object"}),
            success_evidence: vec![EvidenceClaim {
                id: "refunded".into(),
                assertion: "payment.status == refunded".into(),
                accepted_types: vec![EvidenceType::ProviderReceipt],
            }],
            effects,
            idempotency: agentmesh_protocol::Idempotency::KeyRequired,
            recovery: agentmesh_protocol::RecoveryStrategy::Reconcile,
        },
        authority: Default::default(),
        implementations: vec![ImplementationBinding {
            id: "fixture".into(),
            kind: ImplementationKind::Mcp,
            reference: "mcp://fixture/refund".into(),
            configuration: BTreeMap::new(),
        }],
        provenance: Some(Provenance {
            publisher: "acme".into(),
            package_digest: String::new(),
            source: ProvenanceSource::Authored,
            signature: None,
        }),
        extensions: BTreeMap::new(),
    };
    let digest = package.content_digest().expect("package digest");
    package.provenance.as_mut().unwrap().package_digest = digest;
    package
}

#[test]
fn capability_admission_fails_closed_and_requires_approval_for_financial_effects() {
    let package = package(vec![EffectKind::Financial]);
    assert_eq!(capability_risk(&package), Risk::High);

    let denied = admit_capability(&package, &CapabilityAdmissionPolicy::default());
    assert_eq!(denied.effect, PolicyEffect::Deny);
    assert_eq!(denied.reason, "effect_not_allowed");

    let policy = CapabilityAdmissionPolicy {
        allowed_effects: BTreeSet::from([EffectKind::Financial]),
        granted_permissions: BTreeSet::new(),
        maximum_risk: Risk::High,
        approval_at_or_above: Risk::High,
        require_provenance: true,
    };
    let approval = admit_capability(&package, &policy);
    assert_eq!(approval.effect, PolicyEffect::RequireApproval);
}

#[test]
fn unknown_effect_declarations_are_critical_and_not_admitted() {
    let package = package(Vec::new());
    assert_eq!(capability_risk(&package), Risk::Critical);
    assert_eq!(
        admit_capability(&package, &CapabilityAdmissionPolicy::default()).reason,
        "risk_exceeds_policy"
    );
}

struct TestSignatureVerifier;

impl PackageSignatureVerifier for TestSignatureVerifier {
    fn verify(&self, publisher: &str, digest: &str, signature: &str) -> bool {
        publisher == "acme" && digest.starts_with("sha256:") && signature == "valid-test-signature"
    }
}

#[test]
fn publisher_trust_verification_is_separate_from_execution_admission() {
    let mut package = package(vec![EffectKind::ReadOnly]);
    package.provenance.as_mut().unwrap().signature = Some("valid-test-signature".into());
    let digest = package.content_digest().unwrap();
    package.provenance.as_mut().unwrap().package_digest = digest;

    let decision = verify_capability_trust(
        &package,
        &CapabilityTrustPolicy {
            trusted_publishers: BTreeSet::from(["acme".into()]),
            require_signature: true,
        },
        &TestSignatureVerifier,
    );
    assert!(decision.trusted);

    package.capability.intent = "Tampered intent".into();
    let decision = verify_capability_trust(
        &package,
        &CapabilityTrustPolicy::default(),
        &TestSignatureVerifier,
    );
    assert!(!decision.trusted);
}

#[test]
fn approval_is_bound_to_the_values_rendered_for_a_capability_checkpoint() {
    let mut package = package(vec![EffectKind::Financial]);
    package
        .authority
        .approvals
        .push(agentmesh_protocol::ApprovalCheckpoint {
            before: "execute_refund".into(),
            display: vec!["customer".into(), "amount".into()],
            required_role: Some("finance_approver".into()),
        });
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let store = ApprovalStore::default();
    let values = BTreeMap::from([
        ("customer".into(), serde_json::json!("C-123")),
        ("amount".into(), serde_json::json!(125.00)),
    ]);
    let request = store
        .request_capability_checkpoint(
            scope,
            "alice",
            &package,
            "execute_refund",
            &values,
            10_000,
            1,
        )
        .unwrap();
    assert_eq!(request.display_values["customer"], "C-123");
    assert!(capability_approval_matches(
        &request,
        &package,
        "execute_refund",
        &values
    ));

    let changed = BTreeMap::from([
        ("customer".into(), serde_json::json!("C-123")),
        ("amount".into(), serde_json::json!(250.00)),
    ]);
    assert!(!capability_approval_matches(
        &request,
        &package,
        "execute_refund",
        &changed
    ));
}
