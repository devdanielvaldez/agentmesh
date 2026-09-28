//! Circuit state machine integration tests.

use agentmesh_circuit_breaker::{CircuitBreaker, CircuitConfig, CircuitKey, CircuitState};
use agentmesh_error::ErrorCode;
use agentmesh_registry::EndpointId;

fn breaker() -> std::sync::Arc<CircuitBreaker> {
    CircuitBreaker::new(
        CircuitKey {
            endpoint_id: EndpointId::new(),
            capability: Some("weather.current".into()),
        },
        CircuitConfig {
            window_size: 4,
            minimum_throughput: 2,
            failure_basis_points: 5_000,
            open_millis: 100,
            half_open_permits: 1,
            close_after_successes: 2,
        },
    )
    .unwrap()
}

#[test]
fn failures_open_half_open_and_successes_close() {
    let breaker = breaker();
    breaker.acquire(0).unwrap().complete(false, 1);
    breaker.acquire(2).unwrap().complete(false, 3);
    assert_eq!(breaker.snapshot().state, CircuitState::Open);
    assert_eq!(
        breaker.acquire(50).err().unwrap().code(),
        ErrorCode::CircuitOpen
    );
    let first_probe = breaker.acquire(103).unwrap();
    assert_eq!(breaker.snapshot().state, CircuitState::HalfOpen);
    assert_eq!(
        breaker.acquire(104).err().unwrap().code(),
        ErrorCode::CircuitOpen
    );
    first_probe.complete(true, 105);
    breaker.acquire(106).unwrap().complete(true, 107);
    assert_eq!(breaker.snapshot().state, CircuitState::Closed);
}

#[test]
fn failed_probe_reopens_and_dropped_permit_releases_capacity() {
    let breaker = breaker();
    breaker.acquire(0).unwrap().complete(false, 1);
    breaker.acquire(2).unwrap().complete(false, 3);
    drop(breaker.acquire(103).unwrap());
    breaker.acquire(104).unwrap().complete(false, 105);
    assert_eq!(breaker.snapshot().state, CircuitState::Open);
}

#[test]
fn invalid_configuration_and_keys_are_rejected() {
    let invalid = CircuitConfig {
        window_size: 0,
        ..CircuitConfig::default()
    };
    assert_eq!(
        CircuitBreaker::new(
            CircuitKey {
                endpoint_id: EndpointId::new(),
                capability: None
            },
            invalid
        )
        .err()
        .unwrap()
        .code(),
        ErrorCode::ConfigurationInvalid
    );
    assert_eq!(
        CircuitBreaker::new(
            CircuitKey {
                endpoint_id: EndpointId::new(),
                capability: Some(String::new())
            },
            CircuitConfig::default()
        )
        .err()
        .unwrap()
        .code(),
        ErrorCode::ConfigurationInvalid
    );
}
