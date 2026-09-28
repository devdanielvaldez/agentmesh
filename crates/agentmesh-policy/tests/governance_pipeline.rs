//! End-to-end authentication, authorization, policy, limit, and task flow.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use agentmesh_authn::{
    ApiKeyRecord, ApiKeyVerifier, AuthenticationRequest, Authenticator, PresentedCredential,
    SecretCredential,
};
use agentmesh_authz::{
    Action, AuthorizationEngine, AuthorizationRequest, AuthorizationRevision,
    AuthorizationSnapshot, Effect, PermissionRule, Resource, Role, RoleBinding,
};
use agentmesh_policy::{
    ApprovalState, ApprovalStore, PolicyBundle, PolicyContext, PolicyEffect, PolicyMatch,
    PolicyRule, Risk,
};
use agentmesh_rate_limit::{InMemoryRateLimiter, LimitDimension, LimitKey, LimitPolicy};
use agentmesh_registry::{EndpointId, RegistryScope};
use agentmesh_tasks::{InMemoryTaskStore, TaskAccess, TaskRecord};

#[test]
#[allow(clippy::too_many_lines)]
fn governed_call_becomes_an_affine_task_after_independent_approval() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let key_record = ApiKeyRecord::from_plaintext(
        "developer-key",
        &SecretCredential::new("secret-value").unwrap(),
        scope.clone(),
        "alice",
        BTreeSet::from(["developer".into()]),
        None,
    )
    .unwrap();
    let authenticator = Authenticator::new(vec![Arc::new(
        ApiKeyVerifier::new(vec![key_record]).unwrap(),
    )])
    .unwrap();
    let credential = PresentedCredential::ApiKey(SecretCredential::new("secret-value").unwrap());
    let principal = authenticator
        .authenticate(&AuthenticationRequest {
            scope: &scope,
            credential: &credential,
            now_millis: 1,
        })
        .unwrap();

    let authorization = AuthorizationEngine::new(
        AuthorizationSnapshot::compile(
            "acme",
            AuthorizationRevision(2),
            vec![PermissionRule {
                id: "deploy-call".into(),
                effect: Effect::Allow,
                actions: BTreeSet::from([Action::ToolCall]),
                resource_kind: "tool".into(),
                resource_name: "deploy".into(),
                labels: BTreeMap::new(),
            }],
            vec![Role {
                name: "developer".into(),
                permissions: BTreeSet::from(["deploy-call".into()]),
            }],
            vec![RoleBinding {
                id: "developers".into(),
                role: "developer".into(),
                principals: BTreeSet::from(["alice".into()]),
                namespace: Some("prod".into()),
                delegated_users: BTreeSet::new(),
            }],
        )
        .unwrap(),
        16,
    )
    .unwrap();
    let resource = Resource {
        kind: "tool".into(),
        name: "deploy".into(),
        labels: BTreeMap::new(),
    };
    assert!(
        authorization
            .evaluate(&AuthorizationRequest {
                principal: &principal,
                scope: &scope,
                action: &Action::ToolCall,
                resource: &resource,
                delegated_user: None,
            })
            .allowed
    );

    let policies = PolicyBundle::compile(
        "acme",
        3,
        vec![PolicyRule {
            id: "approve-deploy".into(),
            priority: 100,
            matcher: PolicyMatch {
                actions: BTreeSet::from([Action::ToolCall]),
                minimum_risk: Some(Risk::High),
                ..PolicyMatch::default()
            },
            effect: PolicyEffect::RequireApproval,
            reason: "production_change".into(),
            obligations: BTreeSet::from(["audit_required".into()]),
        }],
    )
    .unwrap();
    let argument_metadata = BTreeMap::new();
    let policy = policies.evaluate(&PolicyContext {
        principal: &principal,
        scope: &scope,
        action: &Action::ToolCall,
        resource: &resource,
        environment: "production",
        data_classification: "internal",
        risk: Risk::High,
        minute_utc: 600,
        argument_metadata: &argument_metadata,
    });
    assert_eq!(policy.effect, PolicyEffect::RequireApproval);

    let approvals = ApprovalStore::default();
    let request = approvals
        .request(scope.clone(), "alice", "deploy:release-42", 1_000, 1)
        .unwrap();
    let approval = approvals
        .resolve(&scope, request.id, "release-manager", true, 2)
        .unwrap();
    assert_eq!(approval.state, ApprovalState::Approved);

    let limiter = InMemoryRateLimiter::new(100).unwrap();
    let limit_key = LimitKey::new(
        scope.clone(),
        LimitDimension::Principal,
        principal.id.clone(),
        1,
    )
    .unwrap();
    let limit_lease = limiter
        .acquire(&limit_key, &LimitPolicy::default(), 1, 2)
        .unwrap();

    let tasks = InMemoryTaskStore::new();
    let task = tasks
        .create(
            TaskRecord::new(
                scope.clone(),
                principal.id.clone(),
                EndpointId::new(),
                "backend-task-42",
                10_000,
                2,
            )
            .unwrap(),
            2,
        )
        .unwrap();
    let access = TaskAccess {
        scope: &scope,
        principal: &principal.id,
        administer: false,
    };
    assert_eq!(
        tasks.get(&access, task.id, 3).unwrap().backend_task_id(),
        "backend-task-42"
    );
    drop(limit_lease);
    assert_eq!(limiter.snapshot(&limit_key).unwrap().active, 0);
}
