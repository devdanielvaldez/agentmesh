//! Tenant-safe MCP task identity, backend affinity, and lifecycle management.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{Mutex, MutexGuard},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::{EndpointId, RegistryScope};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum bytes in an upstream task identifier.
pub const MAX_BACKEND_TASK_ID_BYTES: usize = 1_024;
/// Maximum task records retained by the in-memory backend.
pub const MAX_TASKS: usize = 100_000;
/// Maximum undrained lifecycle events.
pub const MAX_TASK_EVENTS: usize = 10_000;

/// Public opaque task identifier exposed through `AgentMesh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(Uuid);

impl TaskId {
    /// Generates a new unpredictable identifier.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Monotonic revision for optimistic task updates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskRevision(u64);

impl TaskRevision {
    /// Initial task revision.
    pub const ZERO: Self = Self(0);

    /// Integer representation.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, AgentMeshError> {
        self.0.checked_add(1).map(Self).ok_or_else(|| {
            AgentMeshError::new(ErrorCode::Conflict, "The task revision space is exhausted.")
        })
    }
}

/// Portable MCP task lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Accepted but not yet executing.
    Pending,
    /// Executing at its affine backend.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished unsuccessfully.
    Failed,
    /// Cancellation was accepted.
    Cancelled,
}

impl TaskStatus {
    /// Whether no further lifecycle transition is allowed.
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// Complete task mapping stored by the task service.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecord {
    /// AgentMesh-facing task identifier.
    pub id: TaskId,
    /// Tenant boundary.
    pub scope: RegistryScope,
    /// Principal that owns the task.
    pub owner_principal: String,
    /// Physical endpoint that owns backend state.
    pub backend_endpoint: EndpointId,
    /// Upstream task ID, never exposed through `Debug`.
    backend_task_id: String,
    /// Current lifecycle.
    pub status: TaskStatus,
    /// Monotonic expiration timestamp.
    pub expires_at_millis: u64,
    /// Optional trace correlation ID.
    pub trace_id: Option<String>,
    /// Optimistic task revision.
    pub revision: TaskRevision,
}

impl fmt::Debug for TaskRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TaskRecord")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("owner_principal", &self.owner_principal)
            .field("backend_endpoint", &self.backend_endpoint)
            .field("backend_task_id", &"[REDACTED]")
            .field("status", &self.status)
            .field("expires_at_millis", &self.expires_at_millis)
            .field("trace_id", &self.trace_id)
            .field("revision", &self.revision)
            .finish()
    }
}

impl TaskRecord {
    /// Creates a pending task mapping after validating all external values.
    ///
    /// # Errors
    ///
    /// Rejects empty, control-character, oversized, or already-expired values.
    pub fn new(
        scope: RegistryScope,
        owner_principal: impl Into<String>,
        backend_endpoint: EndpointId,
        backend_task_id: impl Into<String>,
        expires_at_millis: u64,
        now_millis: u64,
    ) -> Result<Self, AgentMeshError> {
        let record = Self {
            id: TaskId::new(),
            scope,
            owner_principal: owner_principal.into(),
            backend_endpoint,
            backend_task_id: backend_task_id.into(),
            status: TaskStatus::Pending,
            expires_at_millis,
            trace_id: None,
            revision: TaskRevision::ZERO,
        };
        validate_value(&record.owner_principal, 256)?;
        validate_value(&record.backend_task_id, MAX_BACKEND_TASK_ID_BYTES)?;
        if expires_at_millis <= now_millis {
            return Err(invalid("Task expiration must be in the future."));
        }
        Ok(record)
    }

    /// Returns the upstream identifier only to the proxy/task adapter.
    pub fn backend_task_id(&self) -> &str {
        &self.backend_task_id
    }
}

/// Explicit authorization context for every task lookup or mutation.
pub struct TaskAccess<'a> {
    /// Tenant boundary.
    pub scope: &'a RegistryScope,
    /// Authenticated principal.
    pub principal: &'a str,
    /// Administrative task permission within this tenant.
    pub administer: bool,
}

/// Sanitized lifecycle event suitable for telemetry and audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEvent {
    /// Task identifier.
    pub task_id: TaskId,
    /// Tenant boundary.
    pub scope: RegistryScope,
    /// Previous state, absent for creation.
    pub previous: Option<TaskStatus>,
    /// Current state.
    pub current: TaskStatus,
    /// Monotonic event timestamp.
    pub at_millis: u64,
    /// New task revision.
    pub revision: TaskRevision,
}

#[derive(Default)]
struct State {
    tasks: BTreeMap<(RegistryScope, TaskId), TaskRecord>,
    events: VecDeque<TaskEvent>,
}

/// Bounded thread-safe local task backend.
#[derive(Default)]
pub struct InMemoryTaskStore {
    state: Mutex<State>,
}

impl InMemoryTaskStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a mapping and assigns revision one.
    ///
    /// # Errors
    ///
    /// Returns a capacity or conflict error.
    pub fn create(
        &self,
        mut record: TaskRecord,
        now_millis: u64,
    ) -> Result<TaskRecord, AgentMeshError> {
        let mut state = lock(&self.state);
        if state.tasks.len() >= MAX_TASKS {
            return Err(AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The task store is at capacity.",
            ));
        }
        let key = (record.scope.clone(), record.id);
        if state.tasks.contains_key(&key) {
            return Err(conflict());
        }
        record.revision = record.revision.next()?;
        state.tasks.insert(key, record.clone());
        publish(&mut state, &record, None, now_millis);
        Ok(record)
    }

    /// Reads a non-expired task after checking tenant and owner access.
    ///
    /// # Errors
    ///
    /// Returns not-found for missing/expired tasks and permission denied for another owner.
    pub fn get(
        &self,
        access: &TaskAccess<'_>,
        id: TaskId,
        now_millis: u64,
    ) -> Result<TaskRecord, AgentMeshError> {
        let state = lock(&self.state);
        let record = state
            .tasks
            .get(&(access.scope.clone(), id))
            .ok_or_else(not_found)?;
        authorize(record, access)?;
        if record.expires_at_millis <= now_millis {
            return Err(not_found());
        }
        Ok(record.clone())
    }

    /// Performs one valid lifecycle transition under optimistic concurrency.
    ///
    /// # Errors
    ///
    /// Rejects unauthorized, stale, expired, or invalid transitions.
    pub fn transition(
        &self,
        access: &TaskAccess<'_>,
        id: TaskId,
        expected: TaskRevision,
        status: TaskStatus,
        now_millis: u64,
    ) -> Result<TaskRecord, AgentMeshError> {
        let mut state = lock(&self.state);
        let key = (access.scope.clone(), id);
        let record = state.tasks.get_mut(&key).ok_or_else(not_found)?;
        authorize(record, access)?;
        if record.expires_at_millis <= now_millis {
            return Err(not_found());
        }
        if record.revision != expected || !valid_transition(record.status, status) {
            return Err(conflict());
        }
        let previous = record.status;
        record.status = status;
        record.revision = record.revision.next()?;
        let result = record.clone();
        publish(&mut state, &result, Some(previous), now_millis);
        Ok(result)
    }

    /// Cancels a pending or running task with the same authorization checks.
    ///
    /// # Errors
    ///
    /// Returns the same access, revision, expiry, and transition errors as [`Self::transition`].
    pub fn cancel(
        &self,
        access: &TaskAccess<'_>,
        id: TaskId,
        expected: TaskRevision,
        now_millis: u64,
    ) -> Result<TaskRecord, AgentMeshError> {
        self.transition(access, id, expected, TaskStatus::Cancelled, now_millis)
    }

    /// Removes expired terminal records and returns the number removed.
    pub fn purge_expired(&self, now_millis: u64, limit: usize) -> usize {
        let mut state = lock(&self.state);
        let keys = state
            .tasks
            .iter()
            .filter(|(_, record)| {
                record.status.terminal() && record.expires_at_millis <= now_millis
            })
            .take(limit)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let removed = keys.len();
        for key in keys {
            state.tasks.remove(&key);
        }
        removed
    }

    /// Drains lifecycle events in occurrence order.
    pub fn drain_events(&self) -> Vec<TaskEvent> {
        lock(&self.state).events.drain(..).collect()
    }
}

fn valid_transition(from: TaskStatus, to: TaskStatus) -> bool {
    matches!(
        (from, to),
        (
            TaskStatus::Pending,
            TaskStatus::Running | TaskStatus::Cancelled | TaskStatus::Failed
        ) | (
            TaskStatus::Running,
            TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled
        )
    )
}

fn authorize(record: &TaskRecord, access: &TaskAccess<'_>) -> Result<(), AgentMeshError> {
    if &record.scope != access.scope
        || (!access.administer && record.owner_principal != access.principal)
    {
        return Err(AgentMeshError::new(
            ErrorCode::PermissionDenied,
            "The principal cannot access this task.",
        ));
    }
    Ok(())
}

fn publish(state: &mut State, record: &TaskRecord, previous: Option<TaskStatus>, at_millis: u64) {
    if state.events.len() == MAX_TASK_EVENTS {
        state.events.pop_front();
    }
    state.events.push_back(TaskEvent {
        task_id: record.id,
        scope: record.scope.clone(),
        previous,
        current: record.status,
        at_millis,
        revision: record.revision,
    });
}

fn validate_value(value: &str, max: usize) -> Result<(), AgentMeshError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(invalid("A task identity value is invalid or unbounded."));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalid(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::InvalidRequest, message)
}

fn conflict() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::Conflict,
        "The task state or revision conflicts with this operation.",
    )
}

fn not_found() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::CapabilityNotFound,
        "The task does not exist or has expired.",
    )
}
