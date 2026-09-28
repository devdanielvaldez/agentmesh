//! Deterministic, allocation-light routing over immutable registry snapshots.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_core::ServerId;
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::{
    CapabilityKind, Endpoint, EndpointLifecycle, RegistryRevision, RegistryScope, RegistrySnapshot,
    ServiceLifecycle,
};
use serde::{Deserialize, Serialize};

/// Maximum number of rules accepted in one compiled snapshot.
pub const MAX_ROUTE_RULES: usize = 10_000;
/// Maximum bytes accepted in a rule identifier or matching value.
pub const MAX_ROUTE_VALUE_BYTES: usize = 1_024;

/// Normalized, tenant-scoped facts available to route matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteContext<'a> {
    /// Tenant boundary for the request.
    pub scope: &'a RegistryScope,
    /// Exact MCP wire method.
    pub method: &'a str,
    /// Capability family, when the operation targets one.
    pub capability_kind: Option<CapabilityKind>,
    /// Capability name or URI, when present.
    pub capability_name: Option<&'a str>,
    /// Selected virtual MCP server, when present.
    pub virtual_server: Option<&'a str>,
    /// Trusted context labels, such as region or environment.
    pub labels: &'a BTreeMap<String, String>,
}

/// Optional exact-match dimensions for a route.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMatch {
    /// MCP method constraint.
    pub method: Option<String>,
    /// Capability-family constraint.
    pub capability_kind: Option<CapabilityKind>,
    /// Capability-name constraint.
    pub capability_name: Option<String>,
    /// Virtual-server constraint.
    pub virtual_server: Option<String>,
    /// Required trusted labels.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

impl RouteMatch {
    fn matches(&self, context: &RouteContext<'_>) -> bool {
        self.method.as_deref().is_none_or(|v| v == context.method)
            && self
                .capability_kind
                .is_none_or(|v| Some(v) == context.capability_kind)
            && self
                .capability_name
                .as_deref()
                .is_none_or(|v| Some(v) == context.capability_name)
            && self
                .virtual_server
                .as_deref()
                .is_none_or(|v| Some(v) == context.virtual_server)
            && self
                .labels
                .iter()
                .all(|(key, value)| context.labels.get(key) == Some(value))
    }

    fn specificity(&self) -> usize {
        usize::from(self.method.is_some())
            + usize::from(self.capability_kind.is_some())
            + usize::from(self.capability_name.is_some())
            + usize::from(self.virtual_server.is_some())
            + self.labels.len()
    }
}

/// Load-balancing and affinity settings selected by a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficPolicy {
    /// Named load-balancing strategy.
    pub strategy: String,
    /// Optional stable affinity field expected from the request context.
    pub affinity_key: Option<String>,
}

impl Default for TrafficPolicy {
    fn default() -> Self {
        Self {
            strategy: "round_robin".into(),
            affinity_key: None,
        }
    }
}

/// Declarative route compiled into an immutable snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRule {
    /// Stable operator-defined rule identifier.
    pub id: String,
    /// Higher values take precedence.
    pub priority: u32,
    /// Exact matching dimensions.
    #[serde(rename = "match")]
    pub matcher: RouteMatch,
    /// Logical service selected by this rule.
    pub destination: ServerId,
    /// Endpoint labels that candidates must contain.
    #[serde(default)]
    pub endpoint_labels: BTreeMap<String, String>,
    /// Traffic behavior forwarded to load balancing.
    #[serde(default)]
    pub traffic: TrafficPolicy,
}

/// Validated immutable route table for one tenant and registry revision.
#[derive(Debug, Clone)]
pub struct RoutingSnapshot {
    scope: RegistryScope,
    revision: RegistryRevision,
    rules: Vec<RouteRule>,
}

impl RoutingSnapshot {
    /// Compiles and deterministically orders a route table.
    ///
    /// # Errors
    ///
    /// Rejects invalid, duplicate, ambiguous, or unbounded rules.
    pub fn compile(
        scope: RegistryScope,
        revision: RegistryRevision,
        mut rules: Vec<RouteRule>,
    ) -> Result<Self, AgentMeshError> {
        if rules.len() > MAX_ROUTE_RULES {
            return Err(invalid("The route table contains too many rules."));
        }
        let mut ids = BTreeSet::new();
        let mut matches = BTreeMap::new();
        for rule in &rules {
            validate_rule(rule)?;
            if !ids.insert(rule.id.as_str()) {
                return Err(invalid("Route rule identifiers must be unique."));
            }
            if let Some(destination) =
                matches.insert((rule.priority, rule.matcher.clone()), rule.destination)
            {
                if destination != rule.destination {
                    return Err(invalid(
                        "Equal-precedence route rules cannot select different services.",
                    ));
                }
            }
        }
        rules.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| right.matcher.specificity().cmp(&left.matcher.specificity()))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(Self {
            scope,
            revision,
            rules,
        })
    }

    /// Registry revision against which this table was compiled.
    pub const fn revision(&self) -> RegistryRevision {
        self.revision
    }

    /// Resolves a request to an eligible candidate pool.
    ///
    /// # Errors
    ///
    /// Returns a stable routing error when the tenant, rule, service, capability,
    /// or endpoint pool is unavailable.
    pub fn route(
        &self,
        context: &RouteContext<'_>,
        registry: &RegistrySnapshot,
    ) -> Result<RouteDecision, AgentMeshError> {
        if context.scope != &self.scope {
            return Err(AgentMeshError::new(
                ErrorCode::PermissionDenied,
                "The routing snapshot does not belong to this tenant.",
            ));
        }
        if registry.revision < self.revision {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationUnavailable,
                "The registry snapshot is older than the routing snapshot.",
            ));
        }
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.matcher.matches(context))
            .ok_or_else(|| {
                AgentMeshError::new(ErrorCode::RouteNotFound, "No route matched the request.")
            })?;
        let service = registry.service(rule.destination).ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::ServerNotFound,
                "The routed service is unavailable.",
            )
        })?;
        if service.scope != self.scope || service.lifecycle != ServiceLifecycle::Active {
            return Err(AgentMeshError::new(
                ErrorCode::NoHealthyEndpoints,
                "The routed service is not eligible for new traffic.",
            ));
        }
        if let (Some(kind), Some(name)) = (context.capability_kind, context.capability_name) {
            if !service
                .capabilities
                .iter()
                .any(|capability| capability.kind == kind && capability.name == name)
            {
                return Err(AgentMeshError::new(
                    ErrorCode::CapabilityNotFound,
                    "The routed service does not advertise the requested capability.",
                ));
            }
        }
        let candidates = service
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.lifecycle == EndpointLifecycle::Active)
            .filter(|endpoint| labels_match(&rule.endpoint_labels, &endpoint.labels))
            .cloned()
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err(AgentMeshError::new(
                ErrorCode::NoHealthyEndpoints,
                "No endpoint satisfies the selected route.",
            ));
        }
        Ok(RouteDecision {
            rule_id: rule.id.clone(),
            service_id: service.id,
            candidates,
            traffic: rule.traffic.clone(),
            registry_revision: registry.revision,
            explanation: format!(
                "rule '{}' selected service '{}' with {} eligible endpoint(s)",
                rule.id,
                service.name,
                service
                    .endpoints
                    .iter()
                    .filter(|endpoint| endpoint.lifecycle == EndpointLifecycle::Active)
                    .filter(|endpoint| labels_match(&rule.endpoint_labels, &endpoint.labels))
                    .count()
            ),
        })
    }
}

/// Deterministic route result consumed by health and load balancing.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// Matched rule identifier.
    pub rule_id: String,
    /// Selected logical service.
    pub service_id: ServerId,
    /// Administratively eligible endpoint candidates.
    pub candidates: Vec<Endpoint>,
    /// Selected traffic policy.
    pub traffic: TrafficPolicy,
    /// Registry revision used for the decision.
    pub registry_revision: RegistryRevision,
    /// Bounded human-readable explanation without request secrets.
    pub explanation: String,
}

fn validate_rule(rule: &RouteRule) -> Result<(), AgentMeshError> {
    validate_value(&rule.id)?;
    if rule.matcher.capability_name.is_some() != rule.matcher.capability_kind.is_some() {
        return Err(invalid(
            "Capability kind and name route constraints must be specified together.",
        ));
    }
    for value in [
        rule.matcher.method.as_deref(),
        rule.matcher.capability_name.as_deref(),
        rule.matcher.virtual_server.as_deref(),
        Some(rule.traffic.strategy.as_str()),
        rule.traffic.affinity_key.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        validate_value(value)?;
    }
    for (key, value) in rule.matcher.labels.iter().chain(&rule.endpoint_labels) {
        validate_value(key)?;
        validate_value(value)?;
    }
    Ok(())
}

fn validate_value(value: &str) -> Result<(), AgentMeshError> {
    if value.trim().is_empty()
        || value.len() > MAX_ROUTE_VALUE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            "A route value is empty, unbounded, or contains control characters.",
        ));
    }
    Ok(())
}

fn labels_match(required: &BTreeMap<String, String>, actual: &BTreeMap<String, String>) -> bool {
    required
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

fn invalid(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
