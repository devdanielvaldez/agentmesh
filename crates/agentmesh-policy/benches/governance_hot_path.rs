//! Dependency-free baseline benchmark for the governance decision path.

use std::{
    collections::{BTreeMap, BTreeSet},
    hint::black_box,
    sync::Arc,
    time::Instant,
};

use agentmesh_authn::{
    ApiKeyRecord, ApiKeyVerifier, AuthenticationRequest, Authenticator, PresentedCredential,
    SecretCredential,
};
use agentmesh_authz::{
    Action, AuthorizationEngine, AuthorizationRequest, AuthorizationRevision,
    AuthorizationSnapshot, Effect, PermissionRule, Resource, Role, RoleBinding,
};
use agentmesh_policy::{PolicyBundle, PolicyContext, PolicyEffect, PolicyMatch, PolicyRule, Risk};
use agentmesh_rate_limit::{InMemoryRateLimiter, LimitDimension, LimitKey, LimitPolicy};
use agentmesh_registry::{EndpointId, RegistryScope};
use agentmesh_tasks::{InMemoryTaskStore, TaskAccess, TaskRecord};

const ITERATIONS: u64 = 10_000;

#[allow(clippy::too_many_lines)]
fn main() {
    let scope = RegistryScope::new("benchmark", "local").unwrap();
    let secret = SecretCredential::new("benchmark-secret").unwrap();
    let authenticator = Authenticator::new(vec![Arc::new(
        ApiKeyVerifier::new(vec![
            ApiKeyRecord::from_plaintext(
                "benchmark-key",
                &secret,
                scope.clone(),
                "benchmark-principal",
                BTreeSet::from(["caller".into()]),
                None,
            )
            .unwrap(),
        ])
        .unwrap(),
    )])
    .unwrap();
    let credential = PresentedCredential::ApiKey(secret);
    let authorization = AuthorizationEngine::new(
        AuthorizationSnapshot::compile(
            "benchmark",
            AuthorizationRevision(1),
            vec![PermissionRule {
                id: "allow".into(),
                effect: Effect::Allow,
                actions: BTreeSet::from([Action::ToolCall]),
                resource_kind: "tool".into(),
                resource_name: "benchmark.call".into(),
                labels: BTreeMap::new(),
            }],
            vec![Role {
                name: "caller".into(),
                permissions: BTreeSet::from(["allow".into()]),
            }],
            vec![RoleBinding {
                id: "callers".into(),
                role: "caller".into(),
                principals: BTreeSet::from(["benchmark-principal".into()]),
                namespace: Some("local".into()),
                delegated_users: BTreeSet::new(),
            }],
        )
        .unwrap(),
        100,
    )
    .unwrap();
    let policies = PolicyBundle::compile(
        "benchmark",
        1,
        vec![PolicyRule {
            id: "allow-low-risk".into(),
            priority: 1,
            matcher: PolicyMatch {
                actions: BTreeSet::from([Action::ToolCall]),
                maximum_risk: Some(Risk::Low),
                ..PolicyMatch::default()
            },
            effect: PolicyEffect::Allow,
            reason: "low_risk".into(),
            obligations: BTreeSet::new(),
        }],
    )
    .unwrap();
    let limiter = InMemoryRateLimiter::new(10).unwrap();
    let limit_key = LimitKey::new(
        scope.clone(),
        LimitDimension::Principal,
        "benchmark-principal",
        1,
    )
    .unwrap();
    let limit_policy = LimitPolicy {
        rate_capacity: ITERATIONS,
        rate_refill_per_second: ITERATIONS,
        concurrency: 10,
        quota: ITERATIONS,
        quota_window_millis: ITERATIONS + 1,
    };
    let tasks = InMemoryTaskStore::new();
    let task = tasks
        .create(
            TaskRecord::new(
                scope.clone(),
                "benchmark-principal",
                EndpointId::new(),
                "backend-task",
                ITERATIONS + 1,
                0,
            )
            .unwrap(),
            0,
        )
        .unwrap();
    let resource = Resource {
        kind: "tool".into(),
        name: "benchmark.call".into(),
        labels: BTreeMap::new(),
    };
    let metadata = BTreeMap::new();

    let started = Instant::now();
    for iteration in 0..ITERATIONS {
        let principal = authenticator
            .authenticate(&AuthenticationRequest {
                scope: &scope,
                credential: &credential,
                now_millis: iteration,
            })
            .unwrap();
        black_box(authorization.evaluate(&AuthorizationRequest {
            principal: &principal,
            scope: &scope,
            action: &Action::ToolCall,
            resource: &resource,
            delegated_user: None,
        }));
        black_box(policies.evaluate(&PolicyContext {
            principal: &principal,
            scope: &scope,
            action: &Action::ToolCall,
            resource: &resource,
            environment: "local",
            data_classification: "public",
            risk: Risk::Low,
            minute_utc: 1,
            argument_metadata: &metadata,
        }));
        let limit = limiter
            .acquire(&limit_key, &limit_policy, 1, iteration)
            .unwrap();
        black_box(
            tasks
                .get(
                    &TaskAccess {
                        scope: &scope,
                        principal: &principal.id,
                        administer: false,
                    },
                    task.id,
                    iteration,
                )
                .unwrap(),
        );
        drop(limit);
    }
    let elapsed = started.elapsed();
    println!(
        "governance_hot_path: {ITERATIONS} iterations in {elapsed:?} ({:?}/iteration)",
        elapsed / u32::try_from(ITERATIONS).unwrap()
    );
}
