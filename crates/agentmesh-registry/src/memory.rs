//! Lock-protected in-memory registry for standalone and test deployments.

use std::{collections::BTreeMap, sync::RwLock};

use agentmesh_core::ServerId;
use agentmesh_error::{AgentMeshError, ErrorCode};

use crate::{
    RegisteredService, Registry, RegistryRevision, RegistryScope, RegistrySnapshot, WriteCondition,
    model::{MAX_SERVICES_PER_SCOPE, MAX_SNAPSHOT_BYTES, encoded_len},
};

type ServiceKey = (RegistryScope, ServerId);

#[derive(Default)]
struct State {
    revision: RegistryRevision,
    services: BTreeMap<ServiceKey, RegisteredService>,
}

/// Thread-safe, process-local registry implementation.
#[derive(Default)]
pub struct InMemoryRegistry {
    state: RwLock<State>,
}

impl InMemoryRegistry {
    /// Creates an empty in-memory registry.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Registry for InMemoryRegistry {
    fn snapshot(&self, scope: &RegistryScope) -> Result<RegistrySnapshot, AgentMeshError> {
        let state = self.state.read().map_err(|error| lock_error(&error))?;
        let mut services = Vec::new();
        let mut snapshot_bytes = 0_usize;
        for (_, service) in state
            .services
            .range((scope.clone(), ServerId::from_uuid(uuid::Uuid::nil()))..)
            .take_while(|((candidate, _), _)| candidate == scope)
        {
            snapshot_bytes = snapshot_bytes.saturating_add(encoded_len(service)?);
            if snapshot_bytes > MAX_SNAPSHOT_BYTES {
                return Err(AgentMeshError::new(
                    ErrorCode::PayloadTooLarge,
                    "The registry snapshot exceeds its size limit.",
                ));
            }
            services.push(service.clone());
        }
        services.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(RegistrySnapshot {
            revision: state.revision,
            services,
        })
    }

    fn get(
        &self,
        scope: &RegistryScope,
        id: ServerId,
    ) -> Result<Option<RegisteredService>, AgentMeshError> {
        let state = self.state.read().map_err(|error| lock_error(&error))?;
        Ok(state.services.get(&(scope.clone(), id)).cloned())
    }

    fn put(
        &self,
        mut service: RegisteredService,
        condition: WriteCondition,
    ) -> Result<RegisteredService, AgentMeshError> {
        service.validate()?;
        let mut state = self.state.write().map_err(|error| lock_error(&error))?;
        let key = (service.scope.clone(), service.id);
        verify_write_condition(state.services.get(&key), condition)?;
        if state.services.values().any(|stored| {
            stored.scope == service.scope && stored.name == service.name && stored.id != service.id
        }) {
            return Err(conflict("A service with this name already exists."));
        }
        let scope_count = state
            .services
            .keys()
            .filter(|(scope, _)| scope == &service.scope)
            .count();
        if current_is_create(condition) && scope_count >= MAX_SERVICES_PER_SCOPE {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The registry scope reached its service limit.",
            ));
        }
        state.revision = state.revision.next()?;
        service.revision = state.revision;
        state.services.insert(key, service.clone());
        Ok(service)
    }

    fn remove(
        &self,
        scope: &RegistryScope,
        id: ServerId,
        expected: RegistryRevision,
    ) -> Result<RegistryRevision, AgentMeshError> {
        let mut state = self.state.write().map_err(|error| lock_error(&error))?;
        let key = (scope.clone(), id);
        let service = state.services.get(&key).ok_or_else(not_found)?;
        if service.revision != expected {
            return Err(conflict("The service revision does not match."));
        }
        state.revision = state.revision.next()?;
        state.services.remove(&key);
        Ok(state.revision)
    }
}

const fn current_is_create(condition: WriteCondition) -> bool {
    matches!(condition, WriteCondition::Create)
}

pub(crate) fn verify_write_condition(
    current: Option<&RegisteredService>,
    condition: WriteCondition,
) -> Result<(), AgentMeshError> {
    match (current, condition) {
        (None, WriteCondition::Create) => Ok(()),
        (Some(_), WriteCondition::Create) => {
            Err(conflict("The service identifier already exists."))
        }
        (Some(service), WriteCondition::Match(expected)) if service.revision == expected => Ok(()),
        (Some(_), WriteCondition::Match(_)) => {
            Err(conflict("The service revision does not match."))
        }
        (None, WriteCondition::Match(_)) => Err(not_found()),
    }
}

pub(crate) fn conflict(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::Conflict, message)
}

pub(crate) fn not_found() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::ServerNotFound,
        "The registered service does not exist.",
    )
}

fn lock_error<T>(source: &std::sync::PoisonError<T>) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::StorageUnavailable,
        "The in-memory registry is unavailable.",
        std::io::Error::other(source.to_string()),
    )
}
