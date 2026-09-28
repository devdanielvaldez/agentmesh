//! Authentication chain integration tests.

use std::{collections::BTreeSet, sync::Arc};

use agentmesh_authn::{
    ApiKeyRecord, ApiKeyVerifier, AuthenticationRequest, Authenticator, PresentedCredential,
    SecretCredential,
};
use agentmesh_error::ErrorCode;
use agentmesh_registry::RegistryScope;

fn scope(name: &str) -> RegistryScope {
    RegistryScope::new("acme", name).unwrap()
}

#[test]
fn api_keys_authenticate_without_retaining_or_formatting_plaintext() {
    let secret = SecretCredential::new("am_live_secret").unwrap();
    assert!(!format!("{secret:?}").contains("am_live_secret"));
    let record = ApiKeyRecord::from_plaintext(
        "key-1",
        &secret,
        scope("prod"),
        "alice",
        BTreeSet::from(["developer".into()]),
        Some(100),
    )
    .unwrap();
    assert!(!format!("{record:?}").contains("am_live_secret"));
    let auth =
        Authenticator::new(vec![Arc::new(ApiKeyVerifier::new(vec![record]).unwrap())]).unwrap();
    let credential = PresentedCredential::ApiKey(SecretCredential::new("am_live_secret").unwrap());
    let request = AuthenticationRequest {
        scope: &scope("prod"),
        credential: &credential,
        now_millis: 1,
    };
    let principal = auth.authenticate(&request).unwrap();
    assert_eq!(principal.id, "alice");
    assert!(principal.asserted_roles.contains("developer"));
}

#[test]
fn invalid_expired_and_cross_tenant_keys_fail_closed() {
    let record = ApiKeyRecord::from_plaintext(
        "key-1",
        &SecretCredential::new("secret").unwrap(),
        scope("prod"),
        "alice",
        BTreeSet::new(),
        Some(10),
    )
    .unwrap();
    let auth =
        Authenticator::new(vec![Arc::new(ApiKeyVerifier::new(vec![record]).unwrap())]).unwrap();
    for (value, tenant, now, code) in [
        ("wrong", "prod", 1, ErrorCode::InvalidCredentials),
        ("secret", "prod", 10, ErrorCode::InvalidCredentials),
        ("secret", "staging", 1, ErrorCode::PermissionDenied),
    ] {
        let credential = PresentedCredential::ApiKey(SecretCredential::new(value).unwrap());
        let tenant_scope = scope(tenant);
        let request = AuthenticationRequest {
            scope: &tenant_scope,
            credential: &credential,
            now_millis: now,
        };
        assert_eq!(auth.authenticate(&request).unwrap_err().code(), code);
    }
}

#[test]
fn unrelated_mechanisms_report_unauthenticated() {
    let auth =
        Authenticator::new(vec![Arc::new(ApiKeyVerifier::new(Vec::new()).unwrap())]).unwrap();
    let credential = PresentedCredential::MutualTls {
        subject: "client.example".into(),
    };
    let tenant_scope = scope("prod");
    let request = AuthenticationRequest {
        scope: &tenant_scope,
        credential: &credential,
        now_millis: 0,
    };
    assert_eq!(
        auth.authenticate(&request).unwrap_err().code(),
        ErrorCode::Unauthenticated
    );
}
