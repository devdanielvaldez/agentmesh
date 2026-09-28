//! Dependency-free baseline benchmark for the composed traffic hot path.

use std::{collections::BTreeMap, hint::black_box, time::Instant};

use agentmesh_circuit_breaker::{CircuitBreaker, CircuitConfig, CircuitKey};
use agentmesh_core::Transport;
use agentmesh_health::{HealthConfig, HealthManager, HealthObservation, ObservationSource};
use agentmesh_load_balancer::{EndpointPool, Observation, Strategy};
use agentmesh_registry::{
    Capability, CapabilityKind, Endpoint, EndpointId, EndpointLifecycle, RegisteredService,
    RegistryRevision, RegistryScope, RegistrySnapshot,
};
use agentmesh_resilience::Bulkhead;
use agentmesh_router::{RouteContext, RouteMatch, RouteRule, RoutingSnapshot, TrafficPolicy};

const ITERATIONS: u64 = 10_000;

fn main() {
    let scope = RegistryScope::new("benchmark", "local").unwrap();
    let endpoint = Endpoint {
        id: EndpointId::new(),
        transport: Transport::StreamableHttp,
        target: "http://127.0.0.1:3000/mcp".into(),
        lifecycle: EndpointLifecycle::Active,
        weight: 1,
        labels: BTreeMap::new(),
    };
    let endpoint_id = endpoint.id;
    let mut service = RegisteredService::new(scope.clone(), "benchmark");
    let service_id = service.id;
    service.endpoints.push(endpoint.clone());
    service.capabilities.push(Capability {
        kind: CapabilityKind::Tool,
        name: "benchmark.call".into(),
        description: None,
        schema: None,
        integrity: None,
    });
    let registry = RegistrySnapshot {
        revision: RegistryRevision::new(1),
        services: vec![service],
    };
    let routes = RoutingSnapshot::compile(
        scope.clone(),
        registry.revision,
        vec![RouteRule {
            id: "benchmark".into(),
            priority: 1,
            matcher: RouteMatch {
                method: Some("tools/call".into()),
                capability_kind: Some(CapabilityKind::Tool),
                capability_name: Some("benchmark.call".into()),
                ..RouteMatch::default()
            },
            destination: service_id,
            endpoint_labels: BTreeMap::new(),
            traffic: TrafficPolicy::default(),
        }],
    )
    .unwrap();
    let labels = BTreeMap::new();
    let context = RouteContext {
        scope: &scope,
        method: "tools/call",
        capability_kind: Some(CapabilityKind::Tool),
        capability_name: Some("benchmark.call"),
        virtual_server: None,
        labels: &labels,
    };
    let health = HealthManager::new(HealthConfig::default()).unwrap();
    health.record(
        endpoint_id,
        HealthObservation {
            at_millis: 1,
            success: true,
            latency_millis: 1,
            source: ObservationSource::ActiveProbe,
        },
    );
    let pool = EndpointPool::new(vec![endpoint]).unwrap();
    let circuit = CircuitBreaker::new(
        CircuitKey {
            endpoint_id,
            capability: Some("benchmark.call".into()),
        },
        CircuitConfig::default(),
    )
    .unwrap();
    let bulkhead = Bulkhead::new(100).unwrap();

    let started = Instant::now();
    for iteration in 0..ITERATIONS {
        let decision = routes.route(&context, &registry).unwrap();
        black_box(decision);
        black_box(health.snapshot(endpoint_id, 2));
        let mut endpoint_lease = pool.select(Strategy::RoundRobin, None).unwrap();
        let circuit_permit = circuit.acquire(iteration).unwrap();
        let concurrency_permit = bulkhead.try_acquire().unwrap();
        endpoint_lease.observe(Observation {
            latency_micros: 100,
            success: true,
        });
        circuit_permit.complete(true, iteration);
        drop(concurrency_permit);
        drop(endpoint_lease);
    }
    let elapsed = started.elapsed();
    println!(
        "traffic_hot_path: {ITERATIONS} iterations in {elapsed:?} ({:?}/iteration)",
        elapsed / u32::try_from(ITERATIONS).unwrap()
    );
}
