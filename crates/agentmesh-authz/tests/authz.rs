//! RBAC compilation, isolation, deny, delegation, and cache tests.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_authn::{AuthenticatedPrincipal, AuthenticationEvidence, AuthenticationStrength};
use agentmesh_authz::{
    Action, AuthorizationEngine, AuthorizationRequest, AuthorizationRevision,
    AuthorizationSnapshot, Effect, PermissionRule, Resource, Role, RoleBinding,
};
use agentmesh_registry::RegistryScope;

fn fixture() -> (AuthorizationEngine, AuthenticatedPrincipal, RegistryScope) {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let permissions = vec![
        PermissionRule {
            id: "allow-tools".into(),
            effect: Effect::Allow,
            actions: BTreeSet::from([Action::ToolCall, Action::Discover]),
            resource_kind: "tool".into(),
            resource_name: "*".into(),
            labels: BTreeMap::new(),
        },
        PermissionRule {
            id: "deny-danger".into(),
            effect: Effect::Deny,
            actions: BTreeSet::from([Action::ToolCall]),
            resource_kind: "tool".into(),
            resource_name: "danger.delete".into(),
            labels: BTreeMap::new(),
        },
    ];
    let roles = vec![Role {
        name: "developer".into(),
        permissions: BTreeSet::from(["allow-tools".into(), "deny-danger".into()]),
    }];
    let bindings = vec![RoleBinding {
        id: "developers".into(),
        role: "developer".into(),
        principals: BTreeSet::from(["alice".into()]),
        namespace: Some("prod".into()),
        delegated_users: BTreeSet::from(["user-1".into()]),
    }];
    let snapshot = AuthorizationSnapshot::compile(
        "acme",
        AuthorizationRevision(7),
        permissions,
        roles,
        bindings,
    )
    .unwrap();
    let principal = AuthenticatedPrincipal {
        id: "alice".into(),
        scope: scope.clone(),
        strength: AuthenticationStrength::Basic,
        asserted_roles: BTreeSet::new(),
        evidence: AuthenticationEvidence {
            mechanism: "test".into(),
            reference: "fixture".into(),
        },
    };
    (
        AuthorizationEngine::new(snapshot, 32).unwrap(),
        principal,
        scope,
    )
}

fn resource(name: &str) -> Resource {
    Resource {
        kind: "tool".into(),
        name: name.into(),
        labels: BTreeMap::new(),
    }
}

#[test]
fn explicit_deny_wins_and_decisions_are_cached_by_revision() {
    let (engine, principal, scope) = fixture();
    let allowed = resource("weather.current");
    let request = AuthorizationRequest {
        principal: &principal,
        scope: &scope,
        action: &Action::ToolCall,
        resource: &allowed,
        delegated_user: None,
    };
    assert!(engine.evaluate(&request).allowed);
    assert!(engine.evaluate(&request).allowed);
    assert_eq!(engine.cache_metrics(), (1, 1));
    let denied = resource("danger.delete");
    let decision = engine.evaluate(&AuthorizationRequest {
        resource: &denied,
        ..request
    });
    assert!(!decision.allowed);
    assert_eq!(decision.reason, "explicit_deny");
}

#[test]
fn namespace_tenant_and_delegated_user_constraints_fail_closed() {
    let (engine, principal, scope) = fixture();
    let target = resource("weather.current");
    let request = AuthorizationRequest {
        principal: &principal,
        scope: &scope,
        action: &Action::ToolCall,
        resource: &target,
        delegated_user: Some("unknown"),
    };
    assert!(!engine.evaluate(&request).allowed);
    let other_scope = RegistryScope::new("other", "prod").unwrap();
    let request = AuthorizationRequest {
        scope: &other_scope,
        delegated_user: None,
        ..request
    };
    assert_eq!(engine.evaluate(&request).reason, "tenant_mismatch");
}

#[test]
fn visibility_uses_the_same_authorization_path() {
    let (engine, principal, scope) = fixture();
    let resources = vec![resource("weather.current"), resource("danger.delete")];
    let visible = engine.visible(&principal, &scope, &Action::ToolCall, &resources);
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, "weather.current");
}

#[test]
fn compiler_rejects_dangling_role_references() {
    let error = AuthorizationSnapshot::compile(
        "acme",
        AuthorizationRevision(1),
        Vec::new(),
        vec![Role {
            name: "broken".into(),
            permissions: BTreeSet::from(["missing".into()]),
        }],
        Vec::new(),
    )
    .unwrap_err();
    assert_eq!(
        error.code(),
        agentmesh_error::ErrorCode::ConfigurationInvalid
    );
}
