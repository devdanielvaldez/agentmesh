//! Rate, quota, concurrency, isolation, and cleanup integration tests.

use agentmesh_error::ErrorCode;
use agentmesh_rate_limit::{InMemoryRateLimiter, LimitDimension, LimitKey, LimitPolicy};
use agentmesh_registry::RegistryScope;

fn key(namespace: &str, principal: &str) -> LimitKey {
    LimitKey::new(
        RegistryScope::new("acme", namespace).unwrap(),
        LimitDimension::Principal,
        principal,
        1,
    )
    .unwrap()
}

#[test]
fn rate_refills_and_tenant_keys_are_isolated() {
    let limiter = InMemoryRateLimiter::new(10).unwrap();
    let policy = LimitPolicy {
        rate_capacity: 1,
        rate_refill_per_second: 1,
        concurrency: 2,
        quota: 10,
        quota_window_millis: 10_000,
    };
    let alice = key("prod", "alice");
    drop(limiter.acquire(&alice, &policy, 1, 0).unwrap());
    assert_eq!(
        limiter.acquire(&alice, &policy, 1, 0).err().unwrap().code(),
        ErrorCode::RateLimited
    );
    assert!(limiter.acquire(&alice, &policy, 1, 1_000).is_ok());
    let staging = key("staging", "alice");
    assert!(limiter.acquire(&staging, &policy, 1, 0).is_ok());
}

#[test]
fn concurrency_leases_release_on_drop() {
    let limiter = InMemoryRateLimiter::new(10).unwrap();
    let policy = LimitPolicy {
        rate_capacity: 10,
        rate_refill_per_second: 1,
        concurrency: 1,
        quota: 10,
        quota_window_millis: 10_000,
    };
    let key = key("prod", "alice");
    let lease = limiter.acquire(&key, &policy, 1, 0).unwrap();
    assert_eq!(
        limiter.acquire(&key, &policy, 1, 0).err().unwrap().code(),
        ErrorCode::ConcurrencyLimited
    );
    drop(lease);
    assert_eq!(limiter.snapshot(&key).unwrap().active, 0);
}

#[test]
fn quota_resets_only_after_its_window() {
    let limiter = InMemoryRateLimiter::new(10).unwrap();
    let policy = LimitPolicy {
        rate_capacity: 10,
        rate_refill_per_second: 10,
        concurrency: 2,
        quota: 2,
        quota_window_millis: 1_000,
    };
    let key = key("prod", "alice");
    for _ in 0..2 {
        drop(limiter.acquire(&key, &policy, 1, 0).unwrap());
    }
    assert_eq!(
        limiter.acquire(&key, &policy, 1, 1).err().unwrap().code(),
        ErrorCode::QuotaExceeded
    );
    assert!(limiter.acquire(&key, &policy, 1, 1_000).is_ok());
}

#[test]
fn idle_cleanup_never_removes_active_counters() {
    let limiter = InMemoryRateLimiter::new(10).unwrap();
    let key = key("prod", "alice");
    let lease = limiter
        .acquire(&key, &LimitPolicy::default(), 1, 0)
        .unwrap();
    assert_eq!(limiter.purge_idle(100, 10, 10), 0);
    drop(lease);
    assert_eq!(limiter.purge_idle(100, 10, 10), 1);
}
