//! Discovery orchestration, catalog policy, and registry reconciliation.

use agentmesh_core::ServerId;
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::{
    RegisteredService, Registry, RegistryRevision, RegistryScope, ServiceLifecycle, WriteCondition,
};

use crate::{
    DiscoveredCatalog, DiscoveryClient, DiscoveryDiff, DiscoveryLimits, catalog::discover_catalog,
};

/// Policy decision for a changed upstream capability catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeAction {
    /// Store the catalog and keep the service's current lifecycle.
    Accept,
    /// Store the catalog but quarantine the service from routing.
    Quarantine,
    /// Preserve the existing registry document unchanged.
    Reject,
}

/// Synchronous policy boundary for capability catalog changes.
pub trait DiscoveryPolicy: Send + Sync {
    /// Evaluates a normalized change before it reaches the registry.
    fn evaluate(
        &self,
        service: &RegisteredService,
        catalog: &DiscoveredCatalog,
        difference: &DiscoveryDiff,
    ) -> ChangeAction;
}

/// Bootstrap-safe policy that accepts the first catalog and quarantines later changes.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConservativePolicy;

impl DiscoveryPolicy for ConservativePolicy {
    fn evaluate(
        &self,
        service: &RegisteredService,
        _catalog: &DiscoveredCatalog,
        _difference: &DiscoveryDiff,
    ) -> ChangeAction {
        if service.capabilities.is_empty() {
            ChangeAction::Accept
        } else {
            ChangeAction::Quarantine
        }
    }
}

/// Explicit development policy that accepts every valid catalog change.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptAllPolicy;

impl DiscoveryPolicy for AcceptAllPolicy {
    fn evaluate(
        &self,
        _service: &RegisteredService,
        _catalog: &DiscoveredCatalog,
        _difference: &DiscoveryDiff,
    ) -> ChangeAction {
        ChangeAction::Accept
    }
}

/// Effective result of reconciling a discovery run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryDisposition {
    /// A first or policy-approved catalog was stored.
    Accepted,
    /// The changed catalog was stored and the service was quarantined.
    Quarantined,
    /// Policy rejected the change and the stored service was untouched.
    Rejected,
    /// The discovered and stored catalogs were identical.
    Unchanged,
}

/// Auditable summary of one discovery and reconciliation attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryResult {
    /// Normalized upstream catalog and its integrity fingerprint.
    pub catalog: DiscoveredCatalog,
    /// Exact catalog difference evaluated by policy.
    pub difference: DiscoveryDiff,
    /// Resulting registry action.
    pub disposition: DiscoveryDisposition,
    /// Stored service revision after the operation.
    pub service_revision: RegistryRevision,
}

/// Bounded capability discovery engine parameterized by its client and policy.
pub struct DiscoveryEngine<C, P> {
    client: C,
    policy: P,
    limits: DiscoveryLimits,
}

impl<C, P> DiscoveryEngine<C, P>
where
    C: DiscoveryClient,
    P: DiscoveryPolicy,
{
    /// Creates an engine after validating all resource limits.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when any limit is zero.
    pub fn new(client: C, policy: P, limits: DiscoveryLimits) -> Result<Self, AgentMeshError> {
        Ok(Self {
            client,
            policy,
            limits: limits.validate()?,
        })
    }

    /// Fetches and normalizes all supported MCP capability lists.
    ///
    /// # Errors
    ///
    /// Returns an upstream or resource-limit error without mutating registry state.
    pub async fn discover(&self) -> Result<DiscoveredCatalog, AgentMeshError> {
        discover_catalog(&self.client, self.limits).await
    }

    /// Discovers capabilities and atomically reconciles them into one registered service.
    ///
    /// # Errors
    ///
    /// Returns a discovery, policy-independent validation, conflict, or storage error.
    /// The optimistic registry write prevents a stale discovery run from overwriting a
    /// concurrent operator change.
    pub async fn reconcile(
        &self,
        registry: &dyn Registry,
        scope: &RegistryScope,
        service_id: ServerId,
    ) -> Result<DiscoveryResult, AgentMeshError> {
        let mut service = registry.get(scope, service_id)?.ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::ServerNotFound,
                "The registered service does not exist.",
            )
        })?;
        let catalog = self.discover().await?;
        let difference = DiscoveryDiff::between(&service.capabilities, &catalog.capabilities);
        if difference.is_empty() {
            return Ok(DiscoveryResult {
                catalog,
                difference,
                disposition: DiscoveryDisposition::Unchanged,
                service_revision: service.revision,
            });
        }

        let action = self.policy.evaluate(&service, &catalog, &difference);
        if action == ChangeAction::Reject {
            return Ok(DiscoveryResult {
                catalog,
                difference,
                disposition: DiscoveryDisposition::Rejected,
                service_revision: service.revision,
            });
        }

        let expected = service.revision;
        service.capabilities.clone_from(&catalog.capabilities);
        let disposition = if action == ChangeAction::Quarantine {
            service.lifecycle = ServiceLifecycle::Quarantined;
            DiscoveryDisposition::Quarantined
        } else {
            DiscoveryDisposition::Accepted
        };
        let service = registry.put(service, WriteCondition::Match(expected))?;
        Ok(DiscoveryResult {
            catalog,
            difference,
            disposition,
            service_revision: service.revision,
        })
    }
}
