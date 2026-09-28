//! Registry domain model and validation rules.

use std::{collections::BTreeSet, fmt, str::FromStr};

use agentmesh_core::{ServerId, Transport};
use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Maximum services returned by one tenant-scoped snapshot.
pub const MAX_SERVICES_PER_SCOPE: usize = 1_000;
/// Maximum endpoints attached to one logical service.
pub const MAX_ENDPOINTS_PER_SERVICE: usize = 1_000;
/// Maximum capabilities attached to one logical service.
pub const MAX_CAPABILITIES_PER_SERVICE: usize = 10_000;
/// Maximum serialized size of one service document.
pub const MAX_SERVICE_DOCUMENT_BYTES: usize = 256 * 1_024;
/// Maximum cumulative serialized size of one tenant snapshot.
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1_024 * 1_024;

/// Organization and namespace boundary for every registry operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegistryScope {
    organization: String,
    namespace: String,
}

impl RegistryScope {
    /// Creates a validated tenant scope.
    ///
    /// # Errors
    ///
    /// Returns an error when either component is empty, too long, or contains
    /// characters outside ASCII letters, digits, `.`, `_`, and `-`.
    pub fn new(
        organization: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Result<Self, AgentMeshError> {
        let scope = Self {
            organization: organization.into(),
            namespace: namespace.into(),
        };
        validate_name("organization", &scope.organization)?;
        validate_name("namespace", &scope.namespace)?;
        Ok(scope)
    }

    /// Returns the organization identifier.
    pub fn organization(&self) -> &str {
        &self.organization
    }

    /// Returns the namespace identifier.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

/// Monotonic catalog revision used for optimistic concurrency and snapshots.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct RegistryRevision(u64);

impl RegistryRevision {
    /// Initial empty-catalog revision.
    pub const ZERO: Self = Self(0);

    /// Creates a revision from its persisted integer representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the integer representation.
    pub const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn next(self) -> Result<Self, AgentMeshError> {
        self.0.checked_add(1).map(Self).ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The registry revision space is exhausted.",
            )
        })
    }
}

/// Stable identifier for a physical MCP endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EndpointId(Uuid);

impl EndpointId {
    /// Generates a random endpoint identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Creates an endpoint identifier from an existing UUID.
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the underlying UUID.
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for EndpointId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for EndpointId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Administrative lifecycle of a logical MCP service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceLifecycle {
    /// Eligible for normal discovery and routing.
    Active,
    /// Existing work may finish but new work should not be assigned.
    Draining,
    /// Held for operator review and excluded from routing.
    Quarantined,
    /// Administratively disabled.
    Disabled,
}

/// Administrative lifecycle of one physical endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointLifecycle {
    /// Eligible for health evaluation and routing.
    Active,
    /// Existing work may finish but new work should not be assigned.
    Draining,
    /// Held for operator review.
    Quarantined,
    /// Administratively disabled.
    Disabled,
}

/// One physical replica of a logical MCP service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    /// Stable endpoint identifier.
    pub id: EndpointId,
    /// Transport used by this endpoint.
    pub transport: Transport,
    /// HTTP URL or an implementation-specific stdio target.
    pub target: String,
    /// Administrative state independent from runtime health.
    pub lifecycle: EndpointLifecycle,
    /// Relative load-balancing weight.
    pub weight: u32,
    /// Operator-defined matching metadata.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

/// Kind of capability advertised by an MCP service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Callable MCP tool.
    Tool,
    /// Readable MCP resource.
    Resource,
    /// Parameterized MCP resource template.
    ResourceTemplate,
    /// Reusable MCP prompt.
    Prompt,
    /// Long-running task handler.
    Task,
}

/// Normalized capability metadata stored in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    /// Capability family.
    pub kind: CapabilityKind,
    /// Upstream-visible capability name or URI.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Optional MCP input or output schema.
    pub schema: Option<Value>,
    /// Optional discovery-time integrity digest.
    pub integrity: Option<String>,
}

/// Complete logical service document stored atomically by the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredService {
    /// Stable logical service identifier.
    pub id: ServerId,
    /// Tenant boundary that owns this service.
    pub scope: RegistryScope,
    /// Human-readable logical name unique within the scope.
    pub name: String,
    /// Optional operator-facing description.
    pub description: Option<String>,
    /// Administrative service state.
    pub lifecycle: ServiceLifecycle,
    /// Physical replicas.
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// Discovered or declared MCP capabilities.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// Operator-defined routing and ownership metadata.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    /// Last catalog revision that changed this document.
    pub revision: RegistryRevision,
}

impl RegisteredService {
    /// Creates a new active service document at revision zero.
    pub fn new(scope: RegistryScope, name: impl Into<String>) -> Self {
        Self {
            id: ServerId::new(),
            scope,
            name: name.into(),
            description: None,
            lifecycle: ServiceLifecycle::Active,
            endpoints: Vec::new(),
            capabilities: Vec::new(),
            labels: std::collections::BTreeMap::new(),
            revision: RegistryRevision::ZERO,
        }
    }

    /// Validates resource bounds, names, and uniqueness invariants.
    ///
    /// # Errors
    ///
    /// Returns a schema error when the document is not safe to store.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        validate_name("service name", &self.name)?;
        if self
            .description
            .as_ref()
            .is_some_and(|description| description.len() > 4_096)
        {
            return Err(schema_error("The service description is too long."));
        }
        if self.endpoints.len() > MAX_ENDPOINTS_PER_SERVICE {
            return Err(schema_error("The service has too many endpoints."));
        }
        if self.capabilities.len() > MAX_CAPABILITIES_PER_SERVICE {
            return Err(schema_error("The service has too many capabilities."));
        }

        let mut endpoint_ids = BTreeSet::new();
        for endpoint in &self.endpoints {
            if endpoint.target.trim().is_empty() || endpoint.target.len() > 4_096 {
                return Err(schema_error("An endpoint target is invalid."));
            }
            if endpoint.weight == 0 {
                return Err(schema_error("Endpoint weights must be greater than zero."));
            }
            if !endpoint_ids.insert(endpoint.id) {
                return Err(schema_error("Endpoint identifiers must be unique."));
            }
            validate_labels(&endpoint.labels)?;
        }

        let mut capability_keys = BTreeSet::new();
        for capability in &self.capabilities {
            if capability.name.trim().is_empty() || capability.name.len() > 1_024 {
                return Err(schema_error("A capability name is invalid."));
            }
            if !capability_keys.insert((capability.kind, capability.name.as_str())) {
                return Err(schema_error(
                    "Capability kind and name pairs must be unique.",
                ));
            }
            if capability
                .description
                .as_ref()
                .is_some_and(|description| description.len() > 4_096)
            {
                return Err(schema_error("A capability description is too long."));
            }
            if capability
                .integrity
                .as_ref()
                .is_some_and(|digest| digest.len() > 256 || digest.trim().is_empty())
            {
                return Err(schema_error("A capability integrity digest is invalid."));
            }
        }
        validate_labels(&self.labels)?;
        if encoded_len(self)? > MAX_SERVICE_DOCUMENT_BYTES {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The registry service document exceeds its size limit.",
            ));
        }
        Ok(())
    }
}

/// Immutable tenant-scoped view used by routing and inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// Global revision at which this view was read.
    pub revision: RegistryRevision,
    /// Services in stable name-and-ID order.
    pub services: Vec<RegisteredService>,
}

impl RegistrySnapshot {
    /// Finds a service by its stable identifier.
    pub fn service(&self, id: ServerId) -> Option<&RegisteredService> {
        self.services.iter().find(|service| service.id == id)
    }

    /// Returns whether this snapshot is newer than a previously observed revision.
    pub fn changed_since(&self, revision: RegistryRevision) -> bool {
        self.revision > revision
    }

    /// Iterates over services advertising an exact capability kind and name.
    pub fn services_with_capability<'a>(
        &'a self,
        kind: CapabilityKind,
        name: &'a str,
    ) -> impl Iterator<Item = &'a RegisteredService> + 'a {
        self.services.iter().filter(move |service| {
            service
                .capabilities
                .iter()
                .any(|capability| capability.kind == kind && capability.name == name)
        })
    }
}

fn validate_name(field: &str, value: &str) -> Result<(), AgentMeshError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(schema_error(&format!("The registry {field} is invalid.")))
    }
}

fn validate_labels(
    labels: &std::collections::BTreeMap<String, String>,
) -> Result<(), AgentMeshError> {
    if labels.len() > 128
        || labels
            .iter()
            .any(|(key, value)| key.is_empty() || key.len() > 128 || value.len() > 1_024)
    {
        return Err(schema_error(
            "Registry labels exceed their configured bounds.",
        ));
    }
    Ok(())
}

fn schema_error(message: &str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::SchemaInvalid, message)
}

pub(crate) fn encoded_len(service: &RegisteredService) -> Result<usize, AgentMeshError> {
    serde_json::to_vec(service)
        .map(|document| document.len())
        .map_err(|error| {
            AgentMeshError::with_source(
                ErrorCode::SchemaInvalid,
                "The registry service document cannot be encoded.",
                error,
            )
        })
}
