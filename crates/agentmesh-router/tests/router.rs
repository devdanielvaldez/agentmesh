//! Deterministic routing integration tests.

use std::collections::BTreeMap;

use agentmesh_core::Transport;
use agentmesh_error::ErrorCode;
use agentmesh_registry::{
    Capability, CapabilityKind, Endpoint, EndpointId, EndpointLifecycle, RegisteredService,
    RegistryRevision, RegistryScope, RegistrySnapshot,
};
use agentmesh_router::{RouteContext, RouteMatch, RouteRule, RoutingSnapshot, TrafficPolicy};

fn fixture() -> (RegistryScope, RegistrySnapshot, agentmesh_core::ServerId) {
    let scope = RegistryScope::new("acme", "prod").unwrap();
    let mut service = RegisteredService::new(scope.clone(), "weather");
    let id = service.id;
    service.capabilities.push(Capability {
        kind: CapabilityKind::Tool,
        name: "weather.current".into(),
        description: None,
        schema: None,
        integrity: None,
    });
    for region in ["east", "west"] {
        service.endpoints.push(Endpoint {
            id: EndpointId::new(),
            transport: Transport::StreamableHttp,
            target: format!("https://{region}.example/mcp"),
            lifecycle: EndpointLifecycle::Active,
            weight: 1,
            labels: [("region".into(), region.into())].into(),
        });
    }
    (
        scope,
        RegistrySnapshot {
            revision: RegistryRevision::new(4),
            services: vec![service],
        },
        id,
    )
}

fn rule(id: &str, destination: agentmesh_core::ServerId) -> RouteRule {
    RouteRule {
        id: id.into(),
        priority: 10,
        matcher: RouteMatch {
            method: Some("tools/call".into()),
            capability_kind: Some(CapabilityKind::Tool),
            capability_name: Some("weather.current".into()),
            ..RouteMatch::default()
        },
        destination,
        endpoint_labels: [("region".into(), "east".into())].into(),
        traffic: TrafficPolicy::default(),
    }
}

#[test]
fn deterministic_route_filters_service_capability_and_endpoint_labels() {
    let (scope, registry, destination) = fixture();
    let routes = RoutingSnapshot::compile(
        scope.clone(),
        RegistryRevision::new(4),
        vec![rule("weather", destination)],
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
    assert_eq!(decision.rule_id, "weather");
    assert_eq!(decision.candidates.len(), 1);
    assert!(decision.candidates[0].target.contains("east"));
}

#[test]
fn higher_priority_wins_and_equal_conflicts_are_rejected() {
    let (scope, registry, destination) = fixture();
    let mut low = rule("low", destination);
    low.priority = 1;
    low.endpoint_labels.clear();
    let high = rule("high", destination);
    let routes =
        RoutingSnapshot::compile(scope.clone(), RegistryRevision::ZERO, vec![low, high]).unwrap();
    let labels = BTreeMap::new();
    assert_eq!(
        routes
            .route(
                &RouteContext {
                    scope: &scope,
                    method: "tools/call",
                    capability_kind: Some(CapabilityKind::Tool),
                    capability_name: Some("weather.current"),
                    virtual_server: None,
                    labels: &labels
                },
                &registry
            )
            .unwrap()
            .rule_id,
        "high"
    );

    let mut conflicting = rule("other", agentmesh_core::ServerId::new());
    conflicting.endpoint_labels = [("region".into(), "east".into())].into();
    let error = RoutingSnapshot::compile(
        scope,
        RegistryRevision::ZERO,
        vec![rule("one", destination), conflicting],
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ConfigurationInvalid);
}

#[test]
fn tenant_and_revision_boundaries_fail_closed() {
    let (scope, registry, destination) = fixture();
    let routes = RoutingSnapshot::compile(
        scope.clone(),
        RegistryRevision::new(5),
        vec![rule("weather", destination)],
    )
    .unwrap();
    let other = RegistryScope::new("other", "prod").unwrap();
    let labels = BTreeMap::new();
    let context = RouteContext {
        scope: &other,
        method: "tools/call",
        capability_kind: Some(CapabilityKind::Tool),
        capability_name: Some("weather.current"),
        virtual_server: None,
        labels: &labels,
    };
    assert_eq!(
        routes.route(&context, &registry).unwrap_err().code(),
        ErrorCode::PermissionDenied
    );
    let context = RouteContext {
        scope: &scope,
        ..context
    };
    assert_eq!(
        routes.route(&context, &registry).unwrap_err().code(),
        ErrorCode::ConfigurationUnavailable
    );
}

#[test]
fn missing_capability_and_empty_pool_are_explicit() {
    let (scope, mut registry, destination) = fixture();
    let routes = RoutingSnapshot::compile(
        scope.clone(),
        RegistryRevision::ZERO,
        vec![rule("weather", destination)],
    )
    .unwrap();
    let labels = BTreeMap::new();
    let context = RouteContext {
        scope: &scope,
        method: "tools/call",
        capability_kind: Some(CapabilityKind::Tool),
        capability_name: Some("weather.current"),
        virtual_server: None,
        labels: &labels,
    };
    registry.services[0].capabilities.clear();
    assert_eq!(
        routes.route(&context, &registry).unwrap_err().code(),
        ErrorCode::CapabilityNotFound
    );
    registry.services[0].capabilities.push(Capability {
        kind: CapabilityKind::Tool,
        name: "weather.current".into(),
        description: None,
        schema: None,
        integrity: None,
    });
    registry.services[0].endpoints[0].lifecycle = EndpointLifecycle::Draining;
    registry.services[0].endpoints[1].lifecycle = EndpointLifecycle::Disabled;
    assert_eq!(
        routes.route(&context, &registry).unwrap_err().code(),
        ErrorCode::NoHealthyEndpoints
    );
}
