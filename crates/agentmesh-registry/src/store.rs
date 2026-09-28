//! Backend-independent registry contract.

use agentmesh_core::ServerId;
use agentmesh_error::AgentMeshError;

use crate::{RegisteredService, RegistryRevision, RegistryScope, RegistrySnapshot};

/// Optimistic concurrency condition for a service write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteCondition {
    /// The service identifier and name must not already exist in the scope.
    Create,
    /// The stored document must have exactly this revision.
    Match(RegistryRevision),
}

/// Synchronous catalog contract used outside the gateway request hot path.
///
/// Implementations are thread-safe. Consumers should take a snapshot and use
/// that immutable view for routing rather than querying a backend per request.
pub trait Registry: Send + Sync {
    /// Reads all services visible inside one tenant scope.
    ///
    /// # Errors
    ///
    /// Returns a storage error if a consistent snapshot cannot be produced.
    fn snapshot(&self, scope: &RegistryScope) -> Result<RegistrySnapshot, AgentMeshError>;

    /// Reads one service by tenant scope and stable identifier.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the backend cannot complete the read.
    fn get(
        &self,
        scope: &RegistryScope,
        id: ServerId,
    ) -> Result<Option<RegisteredService>, AgentMeshError>;

    /// Creates or replaces a complete service document atomically.
    ///
    /// # Errors
    ///
    /// Returns a validation, conflict, or storage error. A successful write
    /// assigns and returns the next global registry revision.
    fn put(
        &self,
        service: RegisteredService,
        condition: WriteCondition,
    ) -> Result<RegisteredService, AgentMeshError>;

    /// Removes a service only when its stored revision matches `expected`.
    ///
    /// # Errors
    ///
    /// Returns a not-found, conflict, or storage error.
    fn remove(
        &self,
        scope: &RegistryScope,
        id: ServerId,
        expected: RegistryRevision,
    ) -> Result<RegistryRevision, AgentMeshError>;
}
