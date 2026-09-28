//! Credential selection, caching, rotation, redaction, and isolation tests.

use std::{collections::BTreeMap, sync::Arc};

use agentmesh_core::ServerId;
use agentmesh_credentials::{
    CredentialBroker, CredentialRule, InMemorySecretProvider, SecretMaterial, SecretProvider,
    SecretReference,
};
use agentmesh_error::ErrorCode;
use agentmesh_registry::RegistryScope;

#[test]
fn broker_selects_caches_revokes_and_redacts_credentials() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let service = ServerId::new();
    let reference = SecretReference::new("memory", "weather/token", None).unwrap();
    let provider = Arc::new(InMemorySecretProvider::default());
    provider
        .insert(
            reference.clone(),
            &SecretMaterial::new(b"secret-token".to_vec()).unwrap(),
            100,
            "v1",
        )
        .unwrap();
    let provider_dyn: Arc<dyn SecretProvider> = provider.clone();
    let providers: BTreeMap<String, Arc<dyn SecretProvider>> =
        BTreeMap::from([("memory".into(), provider_dyn)]);
    let broker = CredentialBroker::new(
        scope.clone(),
        vec![CredentialRule {
            id: "weather".into(),
            priority: 1,
            service: Some(service),
            capability: Some("weather.current".into()),
            secret: reference.clone(),
        }],
        providers,
    )
    .unwrap();
    let first = broker
        .resolve(&scope, service, Some("weather.current"), 1)
        .unwrap();
    assert_eq!(first.expose(), b"secret-token");
    assert!(!format!("{first:?}").contains("secret-token"));
    provider
        .insert(
            reference.clone(),
            &SecretMaterial::new(b"rotated".to_vec()).unwrap(),
            200,
            "v2",
        )
        .unwrap();
    assert_eq!(
        broker
            .resolve(&scope, service, Some("weather.current"), 2)
            .unwrap()
            .revision,
        "v1"
    );
    assert!(broker.revoke(&reference));
    assert_eq!(
        broker
            .resolve(&scope, service, Some("weather.current"), 3)
            .unwrap()
            .revision,
        "v2"
    );
    let events = broker.drain_events();
    assert_eq!(events.iter().filter(|event| event.cache_hit).count(), 1);
}

#[test]
fn tenant_capability_and_expiry_fail_closed() {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let service = ServerId::new();
    let reference = SecretReference::new("memory", "key", None).unwrap();
    let provider = Arc::new(InMemorySecretProvider::default());
    provider
        .insert(
            reference.clone(),
            &SecretMaterial::new(vec![1]).unwrap(),
            1,
            "v1",
        )
        .unwrap();
    let provider_dyn: Arc<dyn SecretProvider> = provider;
    let providers: BTreeMap<String, Arc<dyn SecretProvider>> =
        BTreeMap::from([("memory".into(), provider_dyn)]);
    let broker = CredentialBroker::new(
        scope.clone(),
        vec![CredentialRule {
            id: "rule".into(),
            priority: 1,
            service: Some(service),
            capability: Some("allowed".into()),
            secret: reference,
        }],
        providers,
    )
    .unwrap();
    assert_eq!(
        broker
            .resolve(&scope, service, Some("missing"), 0)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidCredentials
    );
    let other = RegistryScope::new("other", "prod").unwrap();
    assert_eq!(
        broker
            .resolve(&other, service, Some("allowed"), 0)
            .unwrap_err()
            .code(),
        ErrorCode::PermissionDenied
    );
    assert_eq!(
        broker
            .resolve(&scope, service, Some("allowed"), 1)
            .unwrap_err()
            .code(),
        ErrorCode::CredentialsExpired
    );
}
