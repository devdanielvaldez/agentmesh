//! Integrated credentials, security, cache, telemetry, and audit flow.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr},
    sync::Arc,
};

use agentmesh_audit::{AuditEvent, AuditOutcome, AuditSink, DeliveryPolicy, InMemoryAuditOutbox};
use agentmesh_cache::{CacheKey, CacheKind, CachePolicy, McpCache};
use agentmesh_core::ServerId;
use agentmesh_credentials::{
    CredentialBroker, CredentialRule, InMemorySecretProvider, SecretMaterial, SecretProvider,
    SecretReference,
};
use agentmesh_registry::RegistryScope;
use agentmesh_security::{EgressPolicy, IntegrityDecision, integrity_digest, verify_integrity};
use agentmesh_telemetry::{Attributes, TelemetryRecorder};
use serde_json::json;

#[test]
fn protected_discovery_is_cached_observed_and_audited_without_secret_leakage() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let service = ServerId::new();
    let egress = EgressPolicy::new(
        BTreeSet::from(["mcp.example.com".into()]),
        BTreeSet::from([443]),
        false,
    )
    .unwrap();
    let destination = egress
        .validate_destination(
            "https://mcp.example.com/mcp",
            &[IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
        )
        .unwrap();
    assert_eq!(destination.host(), "mcp.example.com");

    let reference = SecretReference::new("memory", "mcp/token", Some("v1".into())).unwrap();
    let provider = Arc::new(InMemorySecretProvider::default());
    provider
        .insert(
            reference.clone(),
            &SecretMaterial::new(b"upstream-secret".to_vec()).unwrap(),
            1_000,
            "v1",
        )
        .unwrap();
    let provider_dyn: Arc<dyn SecretProvider> = provider;
    let providers: BTreeMap<String, Arc<dyn SecretProvider>> =
        BTreeMap::from([("memory".into(), provider_dyn)]);
    let broker = CredentialBroker::new(
        scope.clone(),
        vec![CredentialRule {
            id: "mcp".into(),
            priority: 1,
            service: Some(service),
            capability: None,
            secret: reference,
        }],
        providers,
    )
    .unwrap();
    let credential = broker.resolve(&scope, service, None, 1).unwrap();
    assert_eq!(credential.expose(), b"upstream-secret");

    let catalog = json!({"tools":[{"name":"weather.current"}]});
    let digest = integrity_digest(&catalog).unwrap();
    assert_eq!(
        verify_integrity(&catalog, Some(&digest)).unwrap(),
        IntegrityDecision::Trusted
    );
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
            100,
            1,
        )
        .unwrap();
    assert!(cache.get(&cache_key, 2).is_some());

    let telemetry = TelemetryRecorder::new(10, 10).unwrap();
    let attributes = Attributes::new(
        BTreeMap::from([("outcome".into(), "success".into())]),
        &BTreeSet::from(["outcome".into()]),
    )
    .unwrap();
    telemetry
        .record("discovery.duration_ms", attributes, 12)
        .unwrap();

    let audit = InMemoryAuditOutbox::new(10).unwrap();
    audit
        .record(
            AuditEvent::new(
                scope,
                "gateway",
                "capability.discovered",
                service.to_string(),
                AuditOutcome::Success,
                None,
                Some(1),
                None,
                2,
                &BTreeMap::from([
                    ("integrity".into(), digest),
                    ("credential_revision".into(), credential.revision.clone()),
                ]),
            )
            .unwrap(),
            DeliveryPolicy::Required,
        )
        .unwrap();
    let encoded = format!("{:?}{:?}", telemetry.metrics(), audit.pending(10));
    assert!(!encoded.contains("upstream-secret"));
    assert_eq!(audit.pending(10).len(), 1);
}
