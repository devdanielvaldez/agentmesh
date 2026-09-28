//! End-to-end composition of routing, health, balancing, circuits, and bulkheads.

use std::collections::BTreeMap;

use agentmesh_circuit_breaker::{CircuitBreaker, CircuitConfig, CircuitKey, CircuitState};
use agentmesh_core::Transport;
use agentmesh_health::{HealthConfig, HealthManager, HealthObservation, ObservationSource};
use agentmesh_load_balancer::{EndpointPool, Observation, Strategy};
use agentmesh_registry::{
    Capability, CapabilityKind, Endpoint, EndpointId, EndpointLifecycle, RegisteredService,
    RegistryRevision, RegistryScope, RegistrySnapshot,
};
use agentmesh_resilience::Bulkhead;
use agentmesh_router::{RouteContext, RouteMatch, RouteRule, RoutingSnapshot, TrafficPolicy};

#[test]
#[allow(clippy::too_many_lines)]
fn unhealthy_replica_is_removed_before_protected_selection() {
    let scope = RegistryScope::new("acme", "production").unwrap();
    let mut service = RegisteredService::new(scope.clone(), "weather");
    service.capabilities.push(Capability {
        kind: CapabilityKind::Tool,
        name: "weather.current".into(),
        description: None,
        schema: None,
        integrity: None,
    });
    for target in ["https://one.example/mcp", "https://two.example/mcp"] {
        service.endpoints.push(Endpoint {
            id: EndpointId::new(),
            transport: Transport::StreamableHttp,
            target: target.into(),
            lifecycle: EndpointLifecycle::Active,
            weight: 1,
            labels: BTreeMap::new(),
        });
    }
    let unhealthy_id = service.endpoints[0].id;
    let healthy_id = service.endpoints[1].id;
    let service_id = service.id;
    let registry = RegistrySnapshot {
        revision: RegistryRevision::new(7),
        services: vec![service],
    };
    let routes = RoutingSnapshot::compile(
        scope.clone(),
        registry.revision,
        vec![RouteRule {
            id: "weather".into(),
            priority: 100,
            matcher: RouteMatch {
                method: Some("tools/call".into()),
                capability_kind: Some(CapabilityKind::Tool),
                capability_name: Some("weather.current".into()),
                ..RouteMatch::default()
            },
            destination: service_id,
            endpoint_labels: BTreeMap::new(),
            traffic: TrafficPolicy {
                strategy: "least_active".into(),
                affinity_key: None,
            },
        }],
    )
    .unwrap();
    let labels = BTreeMap::new();
    let decision = routes
        .route(
            &RouteContext {
                scope: &scope,
                method: "tools/call",
                capability_kind: Some(CapabilityKind::Tool),
                capability_name: Some("weather.current"),
                virtual_server: None,
                labels: &labels,
            },
            &registry,
        )
        .unwrap();

    let health = HealthManager::new(HealthConfig {
        unhealthy_after_failures: 1,
        ..HealthConfig::default()
    })
    .unwrap();
    health.record(
        unhealthy_id,
        HealthObservation {
            at_millis: 1,
            success: false,
            latency_millis: 10,
            source: ObservationSource::PassiveRequest,
        },
    );
    health.record(
        healthy_id,
        HealthObservation {
            at_millis: 1,
            success: true,
            latency_millis: 10,
            source: ObservationSource::ActiveProbe,
        },
    );
    let eligible = decision
        .candidates
        .into_iter()
        .filter(|endpoint| health.snapshot(endpoint.id, 2).state.routable())
        .collect();
    let pool = EndpointPool::new(eligible).unwrap();
    let mut endpoint_lease = pool.select(Strategy::LeastActive, None).unwrap();
    assert_eq!(endpoint_lease.endpoint().id, healthy_id);

    let circuit = CircuitBreaker::new(
        CircuitKey {
            endpoint_id: healthy_id,
            capability: Some("weather.current".into()),
        },
        CircuitConfig::default(),
    )
    .unwrap();
    let circuit_permit = circuit.acquire(2).unwrap();
    let bulkhead = Bulkhead::new(10).unwrap();
    let concurrency_permit = bulkhead.try_acquire().unwrap();

    endpoint_lease.observe(Observation {
        latency_micros: 12_000,
        success: true,
    });
    circuit_permit.complete(true, 14);
    drop(concurrency_permit);
    drop(endpoint_lease);

    assert_eq!(circuit.snapshot().state, CircuitState::Closed);
    assert_eq!(bulkhead.active(), 0);
    assert_eq!(pool.metrics()[0].requests, 1);
}
