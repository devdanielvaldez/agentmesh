//! Cache isolation, expiry, revision, opt-in, and capacity tests.

use agentmesh_cache::{CacheKey, CacheKind, CachePolicy, McpCache};
use agentmesh_error::ErrorCode;
use agentmesh_registry::RegistryScope;

fn key(namespace: &str, identity: &str, revision: u64, kind: CacheKind) -> CacheKey {
    CacheKey {
        scope: RegistryScope::new("acme", namespace).unwrap(),
        kind,
        identity_scope: identity.into(),
        protocol_version: "2026-07-28".into(),
        configuration_revision: revision,
        resource: "weather.current".into(),
    }
}

#[test]
fn tenant_identity_revision_and_expiry_are_exact() {
    let cache = McpCache::new(CachePolicy::default()).unwrap();
    let alice = key("prod", "alice", 1, CacheKind::CapabilityCatalog);
    cache
        .put(alice.clone(), b"catalog".to_vec(), 10, 0)
        .unwrap();
    assert_eq!(cache.get(&alice, 1).unwrap().bytes(), b"catalog");
    assert!(
        cache
            .get(&key("prod", "bob", 1, CacheKind::CapabilityCatalog), 1)
            .is_none()
    );
    assert!(
        cache
            .get(&key("staging", "alice", 1, CacheKind::CapabilityCatalog), 1)
            .is_none()
    );
    assert!(cache.get(&alice, 10).is_none());
    assert_eq!(cache.metrics().hits, 1);
}

#[test]
fn resource_reads_require_explicit_policy_and_tool_calls_are_unrepresentable() {
    let cache = McpCache::new(CachePolicy::default()).unwrap();
    let error = cache
        .put(
            key("prod", "alice", 1, CacheKind::ResourceRead),
            vec![1],
            1,
            0,
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::PolicyDenied);
}

#[test]
fn least_recently_used_entries_are_evicted_with_hard_byte_bounds() {
    let cache = McpCache::new(CachePolicy {
        max_entries: 2,
        max_bytes: 4,
        max_entry_bytes: 2,
        max_ttl_millis: 100,
        allow_resource_reads: true,
    })
    .unwrap();
    let one = key("prod", "one", 1, CacheKind::Discovery);
    let two = key("prod", "two", 1, CacheKind::Discovery);
    let three = key("prod", "three", 1, CacheKind::Discovery);
    cache.put(one.clone(), vec![1, 1], 10, 0).unwrap();
    cache.put(two.clone(), vec![2, 2], 10, 0).unwrap();
    assert!(cache.get(&one, 1).is_some());
    cache.put(three.clone(), vec![3, 3], 10, 1).unwrap();
    assert!(cache.get(&two, 2).is_none());
    assert!(cache.get(&one, 2).is_some());
    assert!(cache.get(&three, 2).is_some());
    assert_eq!(cache.metrics().evictions, 1);
}

#[test]
fn revision_and_scope_invalidation_are_bounded() {
    let cache = McpCache::new(CachePolicy::default()).unwrap();
    let scope = RegistryScope::new("acme", "prod").unwrap();
    for revision in 1..=3 {
        cache
            .put(
                key("prod", "public", revision, CacheKind::PolicyDecision),
                vec![1],
                10,
                0,
            )
            .unwrap();
    }
    assert_eq!(cache.invalidate_before_revision(&scope, 3), 2);
    assert_eq!(cache.invalidate_scope(&scope), 1);
}
