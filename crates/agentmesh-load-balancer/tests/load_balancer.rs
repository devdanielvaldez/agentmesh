//! Endpoint selection and accounting integration tests.

use std::collections::BTreeMap;

use agentmesh_core::Transport;
use agentmesh_error::ErrorCode;
use agentmesh_load_balancer::{EndpointPool, Observation, Strategy};
use agentmesh_registry::{Endpoint, EndpointId, EndpointLifecycle};

fn endpoint(target: &str, weight: u32) -> Endpoint {
    Endpoint {
        id: EndpointId::new(),
        transport: Transport::StreamableHttp,
        target: target.into(),
        lifecycle: EndpointLifecycle::Active,
        weight,
        labels: BTreeMap::default(),
    }
}

#[test]
fn round_robin_is_stable_and_leases_release_active_counts() {
    let pool = EndpointPool::new(vec![endpoint("a", 1), endpoint("b", 1)]).unwrap();
    let first = pool.select(Strategy::RoundRobin, None).unwrap();
    assert_eq!(first.endpoint().target, "a");
    assert_eq!(pool.metrics().iter().map(|m| m.active).sum::<usize>(), 1);
    drop(first);
    assert_eq!(pool.metrics().iter().map(|m| m.active).sum::<usize>(), 0);
    assert_eq!(
        pool.select(Strategy::RoundRobin, None)
            .unwrap()
            .endpoint()
            .target,
        "b"
    );
}

#[test]
fn least_active_and_consistent_hash_preserve_expected_selection() {
    let pool = EndpointPool::new(vec![endpoint("a", 1), endpoint("b", 1)]).unwrap();
    let held = pool.select(Strategy::RoundRobin, None).unwrap();
    assert_eq!(
        pool.select(Strategy::LeastActive, None)
            .unwrap()
            .endpoint()
            .target,
        "b"
    );
    drop(held);
    let one = pool
        .select(Strategy::ConsistentHash, Some(b"session-1"))
        .unwrap();
    let target = one.endpoint().target.clone();
    drop(one);
    assert_eq!(
        pool.select(Strategy::ConsistentHash, Some(b"session-1"))
            .unwrap()
            .endpoint()
            .target,
        target
    );
    assert_eq!(
        pool.select(Strategy::ConsistentHash, None)
            .err()
            .unwrap()
            .code(),
        ErrorCode::ConfigurationInvalid
    );
}

#[test]
fn observations_drive_ewma_and_adaptive_selection() {
    let pool = EndpointPool::new(vec![endpoint("slow", 1), endpoint("fast", 5)]).unwrap();
    for (target, latency, success) in [("slow", 10_000, false), ("fast", 100, true)] {
        let mut lease = loop {
            let lease = pool.select(Strategy::RoundRobin, None).unwrap();
            if lease.endpoint().target == target {
                break lease;
            }
            drop(lease);
        };
        lease.observe(Observation {
            latency_micros: latency,
            success,
        });
    }
    assert_eq!(
        pool.select(Strategy::EwmaLeastLatency, None)
            .unwrap()
            .endpoint()
            .target,
        "fast"
    );
    assert_eq!(
        pool.select(Strategy::Adaptive, None)
            .unwrap()
            .endpoint()
            .target,
        "fast"
    );
    assert_eq!(pool.metrics().iter().map(|m| m.requests).sum::<u64>(), 2);
}

#[test]
fn invalid_pools_fail_before_traffic() {
    assert_eq!(
        EndpointPool::new(Vec::new()).err().unwrap().code(),
        ErrorCode::ConfigurationInvalid
    );
    let mut disabled = endpoint("a", 1);
    disabled.lifecycle = EndpointLifecycle::Disabled;
    assert_eq!(
        EndpointPool::new(vec![disabled]).err().unwrap().code(),
        ErrorCode::ConfigurationInvalid
    );
}
