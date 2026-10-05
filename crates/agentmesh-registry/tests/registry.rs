//! Backend conformance and registry invariant tests.

use agentmesh_core::Transport;
use agentmesh_error::ErrorCode;
use agentmesh_protocol::{
    CAPABILITY_PACKAGE_VERSION, CapabilityContract, CapabilityDefinition, CapabilityPackage,
    EvidenceClaim, EvidenceType, Idempotency, ImplementationBinding, ImplementationKind,
    RecoveryStrategy,
};
use agentmesh_registry::{
    Capability, CapabilityCatalog, CapabilityKind, CapabilityWriteCondition, Endpoint, EndpointId,
    EndpointLifecycle, InMemoryCapabilityCatalog, InMemoryRegistry, RegisteredService, Registry,
    RegistryRevision, RegistryScope, ServiceLifecycle, SqliteCapabilityCatalog, SqliteRegistry,
    WriteCondition,
};
use serde_json::json;

fn scope(name: &str) -> RegistryScope {
    RegistryScope::new("acme", name).expect("valid scope")
}

fn service(scope: RegistryScope, name: &str) -> RegisteredService {
    let mut service = RegisteredService::new(scope, name);
    service.description = Some("Weather tools".into());
    service.endpoints.push(Endpoint {
        id: EndpointId::new(),
        transport: Transport::StreamableHttp,
        target: "https://weather.example.com/mcp".into(),
        lifecycle: EndpointLifecycle::Active,
        weight: 100,
        labels: [("region".into(), "us-east".into())].into(),
    });
    service.capabilities.push(Capability {
        kind: CapabilityKind::Tool,
        name: "weather.current".into(),
        description: Some("Current weather".into()),
        schema: Some(json!({"type": "object"})),
        integrity: Some("sha256:abc123".into()),
    });
    service
}

fn capability_package() -> CapabilityPackage {
    CapabilityPackage {
        schema_version: CAPABILITY_PACKAGE_VERSION.into(),
        capability: CapabilityDefinition {
            id: "weather.current".into(),
            version: "1.0.0".into(),
            intent: "Read current weather".into(),
        },
        contract: CapabilityContract {
            requires: Vec::new(),
            inputs: json!({"type":"object"}),
            outputs: json!({"type":"object"}),
            success_evidence: vec![EvidenceClaim {
                id: "weather_returned".into(),
                assertion: "weather result is present".into(),
                accepted_types: vec![EvidenceType::StateAssertion],
            }],
            effects: vec![agentmesh_protocol::EffectKind::ReadOnly],
            idempotency: Idempotency::Guaranteed,
            recovery: RecoveryStrategy::Retry,
        },
        authority: Default::default(),
        implementations: vec![ImplementationBinding {
            id: "weather-mcp".into(),
            kind: ImplementationKind::Mcp,
            reference: "mcp://weather/current".into(),
            configuration: Default::default(),
        }],
        provenance: None,
        extensions: Default::default(),
    }
}

#[test]
fn capability_catalog_enforces_tenant_scope_and_optimistic_revisions() {
    let catalog = InMemoryCapabilityCatalog::new();
    let production = scope("production");
    let created = catalog
        .publish(
            production.clone(),
            capability_package(),
            CapabilityWriteCondition::Create,
        )
        .expect("publish package");
    assert_eq!(created.revision, RegistryRevision::new(1));
    assert!(
        catalog
            .get(&scope("staging"), "weather.current", "1.0.0")
            .expect("tenant lookup")
            .is_none()
    );

    let duplicate = catalog
        .publish(
            production.clone(),
            capability_package(),
            CapabilityWriteCondition::Create,
        )
        .expect_err("duplicate package version");
    assert_eq!(duplicate.code(), ErrorCode::Conflict);

    let revision = catalog
        .revoke(&production, "weather.current", "1.0.0", created.revision)
        .expect("revoke package");
    assert_eq!(revision, RegistryRevision::new(2));
}

#[test]
fn sqlite_capability_catalog_persists_revisioned_packages() {
    let directory = tempfile::tempdir().expect("temporary catalog directory");
    let path = directory.path().join("capabilities.sqlite");
    let scope = scope("production");
    let first = SqliteCapabilityCatalog::open(&path).expect("open catalog");
    let published = first
        .publish(
            scope.clone(),
            capability_package(),
            CapabilityWriteCondition::Create,
        )
        .expect("publish capability");
    drop(first);

    let reopened = SqliteCapabilityCatalog::open(&path).expect("reopen catalog");
    let snapshot = reopened.snapshot(&scope).expect("snapshot");
    assert_eq!(snapshot.revision, published.revision);
    assert_eq!(snapshot.capabilities.len(), 1);
    assert_eq!(
        snapshot.capabilities[0].package.capability.id,
        "weather.current"
    );
}

fn assert_backend_contract(registry: &dyn Registry) {
    let production = scope("production");
    let staging = scope("staging");
    let created = registry
        .put(
            service(production.clone(), "weather"),
            WriteCondition::Create,
        )
        .expect("register service");
    assert_eq!(created.revision, RegistryRevision::new(1));

    let duplicate = service(production.clone(), "weather");
    let error = registry
        .put(duplicate, WriteCondition::Create)
        .expect_err("duplicate name");
    assert_eq!(error.code(), ErrorCode::Conflict);

    let same_name_other_scope = registry
        .put(service(staging.clone(), "weather"), WriteCondition::Create)
        .expect("tenant-isolated name");
    assert_eq!(same_name_other_scope.revision, RegistryRevision::new(2));

    let mut updated = created.clone();
    updated.lifecycle = ServiceLifecycle::Draining;
    let updated = registry
        .put(updated, WriteCondition::Match(created.revision))
        .expect("optimistic update");
    assert_eq!(updated.revision, RegistryRevision::new(3));

    let stale_error = registry
        .put(created, WriteCondition::Match(RegistryRevision::new(1)))
        .expect_err("stale update");
    assert_eq!(stale_error.code(), ErrorCode::Conflict);

    let snapshot = registry.snapshot(&production).expect("snapshot");
    assert_eq!(snapshot.revision, RegistryRevision::new(3));
    assert_eq!(snapshot.services.len(), 1);
    assert_eq!(snapshot.services[0].lifecycle, ServiceLifecycle::Draining);
    assert!(snapshot.changed_since(RegistryRevision::new(2)));
    assert_eq!(
        snapshot
            .services_with_capability(CapabilityKind::Tool, "weather.current")
            .count(),
        1
    );

    let removal_revision = registry
        .remove(&production, updated.id, updated.revision)
        .expect("remove service");
    assert_eq!(removal_revision, RegistryRevision::new(4));
    assert!(
        registry
            .get(&production, updated.id)
            .expect("read removed service")
            .is_none()
    );
    assert_eq!(
        registry
            .snapshot(&staging)
            .expect("isolated snapshot")
            .services
            .len(),
        1
    );
}

#[test]
fn in_memory_backend_obeys_registry_contract() {
    assert_backend_contract(&InMemoryRegistry::new());
}

#[test]
fn sqlite_backend_obeys_registry_contract() {
    assert_backend_contract(&SqliteRegistry::in_memory().expect("SQLite registry"));
}

#[test]
fn sqlite_registry_persists_service_documents_and_revisions() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("registry.db");
    let production = scope("production");
    let id;
    {
        let registry = SqliteRegistry::open(&path).expect("open registry");
        let created = registry
            .put(
                service(production.clone(), "weather"),
                WriteCondition::Create,
            )
            .expect("register service");
        id = created.id;
    }

    let reopened = SqliteRegistry::open(&path).expect("reopen registry");
    let stored = reopened
        .get(&production, id)
        .expect("read persisted service")
        .expect("service exists");
    assert_eq!(stored.name, "weather");
    assert_eq!(stored.revision, RegistryRevision::new(1));
    assert_eq!(stored.capabilities[0].name, "weather.current");
}

#[test]
fn registry_rejects_duplicate_endpoint_and_capability_keys() {
    let mut invalid = service(scope("production"), "weather");
    invalid.endpoints.push(invalid.endpoints[0].clone());
    invalid.capabilities.push(invalid.capabilities[0].clone());

    let error = InMemoryRegistry::new()
        .put(invalid, WriteCondition::Create)
        .expect_err("invalid service");
    assert_eq!(error.code(), ErrorCode::SchemaInvalid);
}

#[test]
fn scope_rejects_ambiguous_or_unbounded_names() {
    assert!(RegistryScope::new("", "default").is_err());
    assert!(RegistryScope::new("acme/other", "default").is_err());
    assert!(RegistryScope::new("acme", "x".repeat(129)).is_err());
}

#[test]
fn registry_rejects_oversized_service_documents() {
    let mut oversized = service(scope("production"), "weather");
    oversized.capabilities[0].schema = Some(json!({"value": "x".repeat(300_000)}));

    let error = InMemoryRegistry::new()
        .put(oversized, WriteCondition::Create)
        .expect_err("oversized service");
    assert_eq!(error.code(), ErrorCode::PayloadTooLarge);
}
