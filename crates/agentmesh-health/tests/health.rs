//! Health transition integration tests.

use agentmesh_error::ErrorCode;
use agentmesh_health::{
    HealthConfig, HealthManager, HealthObservation, HealthState, ObservationSource,
};
use agentmesh_registry::EndpointId;

fn observation(at_millis: u64, success: bool) -> HealthObservation {
    HealthObservation {
        at_millis,
        success,
        latency_millis: 10,
        source: ObservationSource::PassiveRequest,
    }
}

#[test]
fn failures_eject_and_successes_warm_up_before_recovery() {
    let manager = HealthManager::new(HealthConfig {
        unhealthy_after_failures: 2,
        recovery_successes: 2,
        ..HealthConfig::default()
    })
    .unwrap();
    let endpoint = EndpointId::new();
    assert_eq!(
        manager.record(endpoint, observation(1, true)),
        HealthState::Healthy
    );
    assert_eq!(
        manager.record(endpoint, observation(2, false)),
        HealthState::Degraded
    );
    assert_eq!(
        manager.record(endpoint, observation(3, false)),
        HealthState::Unhealthy
    );
    assert_eq!(
        manager.record(endpoint, observation(4, true)),
        HealthState::Recovering
    );
    assert_eq!(
        manager.record(endpoint, observation(5, true)),
        HealthState::Degraded
    );
    assert!(manager.drain_events().len() >= 4);
}

#[test]
fn administrative_quarantine_cannot_be_cleared_by_probe() {
    let manager = HealthManager::new(HealthConfig::default()).unwrap();
    let endpoint = EndpointId::new();
    manager
        .set_administrative(endpoint, HealthState::Quarantined, 1)
        .unwrap();
    assert_eq!(
        manager.record(endpoint, observation(2, true)),
        HealthState::Quarantined
    );
    assert!(!manager.snapshot(endpoint, 2).state.routable());
}

#[test]
fn stale_signals_fail_closed_and_probe_jitter_is_stable() {
    let config = HealthConfig {
        stale_after_millis: 20_000,
        ..HealthConfig::default()
    };
    let manager = HealthManager::new(config).unwrap();
    let endpoint = EndpointId::new();
    manager.record(endpoint, observation(1, true));
    assert_eq!(
        manager.snapshot(endpoint, 20_002).state,
        HealthState::Unknown
    );
    assert_eq!(
        manager.next_probe_millis(endpoint, 100),
        manager.next_probe_millis(endpoint, 100)
    );
}

#[test]
fn invalid_configuration_is_rejected() {
    let error = HealthManager::new(HealthConfig {
        window_size: 0,
        ..HealthConfig::default()
    })
    .err()
    .unwrap();
    assert_eq!(error.code(), ErrorCode::ConfigurationInvalid);
}
