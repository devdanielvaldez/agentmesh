//! Desired-state compilation into immutable, integrity-protected gateway snapshots.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_storage::{DocumentStore, StorageScope, WriteCondition};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

/// Maximum resources compiled into one snapshot.
pub const MAX_SNAPSHOT_RESOURCES: usize = 50_000;
/// Maximum serialized snapshot size.
pub const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;

/// Lifecycle of a desired-state resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceState {
    /// Resource participates in snapshots.
    Active,
    /// Resource remains present while traffic drains.
    Draining,
    /// Resource is excluded from snapshots.
    Disabled,
}

/// Versioned desired resource accepted by the control plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesiredResource {
    /// Resource kind such as `route` or `policy`.
    pub kind: String,
    /// Stable resource name.
    pub name: String,
    /// Administrative lifecycle.
    pub state: ResourceState,
    /// Domain-specific normalized specification.
    pub spec: serde_json::Value,
}

impl DesiredResource {
    /// Validates bounded identifiers and JSON complexity.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid names or an oversized specification.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if !valid_name(&self.kind) || !valid_name(&self.name) {
            return Err(invalid("Control-plane resource names are invalid."));
        }
        let bytes = serde_json::to_vec(&self.spec).map_err(internal)?;
        if bytes.len() > 1024 * 1024 || depth(&self.spec) > 32 {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The desired resource exceeds its bounds.",
            ));
        }
        Ok(())
    }
}

/// Immutable gateway configuration artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    /// Monotonic scope-local revision.
    pub revision: u64,
    /// Canonically ordered active resources.
    pub resources: Vec<DesiredResource>,
    /// SHA-256 digest over revision and resources.
    pub digest: String,
}

impl ConfigSnapshot {
    /// Verifies that the payload still matches its digest.
    pub fn verify(&self) -> bool {
        self.digest == digest(self.revision, &self.resources).unwrap_or_default()
    }
}

/// Result reported by a gateway applying a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutStatus {
    /// Gateway member identifier.
    pub gateway: String,
    /// Last accepted snapshot revision.
    pub revision: u64,
    /// Whether validation and activation succeeded.
    pub accepted: bool,
    /// Disclosure-safe status reason.
    pub reason: String,
}

/// Compiles persisted desired state and records rollout progress.
pub struct ControlPlane<S> {
    store: S,
    active: Arc<RwLock<BTreeMap<StorageScope, ConfigSnapshot>>>,
    rollouts: Arc<RwLock<BTreeMap<(StorageScope, String), RolloutStatus>>>,
}

impl<S: DocumentStore> ControlPlane<S> {
    /// Creates a control plane using a repository implementation.
    pub fn new(store: S) -> Self {
        Self {
            store,
            active: Arc::default(),
            rollouts: Arc::default(),
        }
    }

    /// Stores a desired resource using optimistic concurrency.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for validation, conflicts, or persistence failures.
    pub fn apply(
        &self,
        scope: &StorageScope,
        resource: DesiredResource,
        condition: WriteCondition,
    ) -> Result<u64, AgentMeshError> {
        resource.validate()?;
        let key = format!("desired/{}/{}", resource.kind, resource.name);
        let value = serde_json::to_value(resource).map_err(internal)?;
        Ok(self.store.put(scope, &key, value, condition)?.revision)
    }

    /// Compiles, validates, and atomically activates the next snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when desired state is invalid or exceeds bounds.
    pub fn reconcile(&self, scope: &StorageScope) -> Result<ConfigSnapshot, AgentMeshError> {
        let documents = self.store.list(scope, "desired/", MAX_SNAPSHOT_RESOURCES)?;
        if documents.len() == MAX_SNAPSHOT_RESOURCES {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The snapshot resource limit was reached.",
            ));
        }
        let mut resources = documents
            .into_iter()
            .map(|document| {
                serde_json::from_value::<DesiredResource>(document.value).map_err(internal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        resources.retain(|resource| resource.state != ResourceState::Disabled);
        resources.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));
        for resource in &resources {
            resource.validate()?;
        }
        let revision = self
            .active
            .read()
            .map_err(|_| unavailable())?
            .get(scope)
            .map_or(1, |snapshot| snapshot.revision.saturating_add(1));
        let digest = digest(revision, &resources)?;
        let snapshot = ConfigSnapshot {
            revision,
            resources,
            digest,
        };
        if serde_json::to_vec(&snapshot).map_err(internal)?.len() > MAX_SNAPSHOT_BYTES {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The compiled snapshot exceeds its size limit.",
            ));
        }
        self.active
            .write()
            .map_err(|_| unavailable())?
            .insert(scope.clone(), snapshot.clone());
        Ok(snapshot)
    }

    /// Returns the active snapshot without querying storage.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if active state is unavailable.
    pub fn active(&self, scope: &StorageScope) -> Result<Option<ConfigSnapshot>, AgentMeshError> {
        Ok(self
            .active
            .read()
            .map_err(|_| unavailable())?
            .get(scope)
            .cloned())
    }

    /// Records bounded gateway acknowledgement state.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for invalid status data or unavailable state.
    pub fn acknowledge(
        &self,
        scope: &StorageScope,
        status: RolloutStatus,
    ) -> Result<(), AgentMeshError> {
        if !valid_name(&status.gateway) || status.reason.len() > 512 {
            return Err(invalid("The rollout status is invalid."));
        }
        self.rollouts
            .write()
            .map_err(|_| unavailable())?
            .insert((scope.clone(), status.gateway.clone()), status);
        Ok(())
    }

    /// Returns rollout states in stable gateway order.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if rollout state is unavailable.
    pub fn rollout_status(
        &self,
        scope: &StorageScope,
    ) -> Result<Vec<RolloutStatus>, AgentMeshError> {
        Ok(self
            .rollouts
            .read()
            .map_err(|_| unavailable())?
            .iter()
            .filter(|((candidate, _), _)| candidate == scope)
            .map(|(_, status)| status.clone())
            .collect())
    }
}

fn digest(revision: u64, resources: &[DesiredResource]) -> Result<String, AgentMeshError> {
    let mut hasher = Sha256::new();
    hasher.update(revision.to_be_bytes());
    hasher.update(serde_json::to_vec(resources).map_err(internal)?);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}
fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}
fn depth(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        serde_json::Value::Object(items) => 1 + items.values().map(depth).max().unwrap_or(0),
        _ => 1,
    }
}
fn invalid(message: &str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::SchemaInvalid, message)
}
fn unavailable() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::ConfigurationUnavailable,
        "Control-plane state is unavailable.",
    )
}
fn internal(error: impl std::error::Error + Send + Sync + 'static) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::ConfigurationInvalid,
        "Control-plane resource encoding failed.",
        error,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_storage::InMemoryStore;
    use serde_json::json;

    #[test]
    fn compiles_deterministic_verified_snapshots_and_rollouts() {
        let scope = StorageScope::new("acme", "prod").unwrap();
        let plane = ControlPlane::new(InMemoryStore::default());
        plane
            .apply(
                &scope,
                DesiredResource {
                    kind: "route".into(),
                    name: "main".into(),
                    state: ResourceState::Active,
                    spec: json!({"service":"search"}),
                },
                WriteCondition::Create,
            )
            .unwrap();
        let first = plane.reconcile(&scope).unwrap();
        assert!(first.verify());
        assert_eq!(plane.active(&scope).unwrap(), Some(first.clone()));
        plane
            .acknowledge(
                &scope,
                RolloutStatus {
                    gateway: "gw-1".into(),
                    revision: first.revision,
                    accepted: true,
                    reason: "activated".into(),
                },
            )
            .unwrap();
        assert_eq!(plane.rollout_status(&scope).unwrap().len(), 1);
    }
}
