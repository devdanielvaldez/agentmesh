//! Dependency-free baseline benchmark for security and observability primitives.

use std::{
    collections::{BTreeMap, BTreeSet},
    hint::black_box,
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
    time::Instant,
};

use agentmesh_audit::{AuditEvent, AuditOutcome, AuditSink, DeliveryPolicy, InMemoryAuditOutbox};
use agentmesh_cache::{CacheKey, CacheKind, CachePolicy, McpCache};
use agentmesh_core::ServerId;
use agentmesh_credentials::{
    CredentialBroker, CredentialRule, InMemorySecretProvider, SecretMaterial, SecretProvider,
    SecretReference,
};
use agentmesh_registry::RegistryScope;
use agentmesh_security::{EgressPolicy, integrity_digest, verify_integrity};
use agentmesh_telemetry::{Attributes, TelemetryRecorder};
use serde_json::json;

const ITERATIONS: u64 = 10_000;

#[allow(clippy::too_many_lines)]
fn main() {
    let scope = RegistryScope::new("benchmark", "local").unwrap();
    let service = ServerId::new();
    let egress = EgressPolicy::new(
        BTreeSet::from(["mcp.example.com".into()]),
        BTreeSet::from([443]),
        false,
    )
    .unwrap();
    let address = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
    let reference = SecretReference::new("memory", "benchmark/token", None).unwrap();
    let provider = Arc::new(InMemorySecretProvider::default());
    provider
        .insert(
            reference.clone(),
            &SecretMaterial::new(b"benchmark-secret".to_vec()).unwrap(),
            ITERATIONS + 1,
            "v1",
        )
        .unwrap();
    let provider_dyn: Arc<dyn SecretProvider> = provider;
    let broker = CredentialBroker::new(
        scope.clone(),
        vec![CredentialRule {
            id: "benchmark".into(),
            priority: 1,
            service: Some(service),
            capability: None,
            secret: reference,
        }],
        BTreeMap::from([("memory".into(), provider_dyn)]),
    )
    .unwrap();
    let catalog = json!({"tools":[{"name":"benchmark.call"}]});
    let digest = integrity_digest(&catalog).unwrap();
    let cache = McpCache::new(CachePolicy::default()).unwrap();
    let cache_key = CacheKey {
        scope: scope.clone(),
        kind: CacheKind::CapabilityCatalog,
        identity_scope: "public".into(),
        protocol_version: "2026-07-28".into(),
        configuration_revision: 1,
        resource: service.to_string(),
    };
    cache
        .put(
            cache_key.clone(),
            serde_json::to_vec(&catalog).unwrap(),
            ITERATIONS + 1,
            0,
        )
        .unwrap();
    let telemetry = TelemetryRecorder::new(10, 10).unwrap();
    let attributes = Attributes::new(
        BTreeMap::from([("outcome".into(), "success".into())]),
        &BTreeSet::from(["outcome".into()]),
    )
    .unwrap();
    let audit = InMemoryAuditOutbox::new(usize::try_from(ITERATIONS).unwrap()).unwrap();
    let metadata = BTreeMap::from([("integrity".into(), digest.clone())]);

    let started = Instant::now();
    for iteration in 0..ITERATIONS {
        black_box(
            egress
                .validate_destination("https://mcp.example.com/mcp", &[address])
                .unwrap(),
        );
        black_box(broker.resolve(&scope, service, None, iteration).unwrap());
        black_box(verify_integrity(&catalog, Some(&digest)).unwrap());
        black_box(cache.get(&cache_key, iteration).unwrap());
        telemetry
            .record("security.operation", attributes.clone(), 1)
            .unwrap();
        audit
            .record(
                AuditEvent::new(
                    scope.clone(),
                    "gateway",
                    "security.operation",
                    service.to_string(),
                    AuditOutcome::Success,
                    None,
                    Some(1),
                    None,
                    iteration,
                    &metadata,
                )
                .unwrap(),
                DeliveryPolicy::Required,
            )
            .unwrap();
    }
    let elapsed = started.elapsed();
    println!(
        "security_observability_hot_path: {ITERATIONS} iterations in {elapsed:?} ({:?}/iteration)",
        elapsed / u32::try_from(ITERATIONS).unwrap()
    );
}
