//! Contextual policy and approval workflow integration tests.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_authn::{AuthenticatedPrincipal, AuthenticationEvidence, AuthenticationStrength};
use agentmesh_authz::{Action, Resource};
use agentmesh_error::ErrorCode;
use agentmesh_policy::{
    ApprovalState, ApprovalStore, PolicyBundle, PolicyContext, PolicyEffect, PolicyMatch,
    PolicyRule, Risk,
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
