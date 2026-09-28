//! Bounded tamper-evident audit records with explicit fail-open/fail-closed delivery policy.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Mutex, MutexGuard},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use agentmesh_security::redact_metadata;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Maximum metadata fields per event.
pub const MAX_AUDIT_METADATA: usize = 64;
/// Maximum outbox capacity.
pub const MAX_AUDIT_RECORDS: usize = 1_000_000;

/// Audit action outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// Action completed successfully.
    Success,
    /// A security or policy decision denied the action.
    Denied,
    /// Action was permitted but failed.
    Failed,
    /// Action is waiting for completion or approval.
    Pending,
}

/// Validated durable audit event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Globally unique event ID.
    pub id: Uuid,
    /// Tenant boundary.
    pub scope: RegistryScope,
    /// Authenticated actor or internal service account.
    pub actor: String,
    /// Stable action.
    pub action: String,
    /// Stable target reference.
    pub target: String,
    /// Result.
    pub outcome: AuditOutcome,
    /// Policy revision, when applicable.
    pub policy_revision: Option<u64>,
    /// Configuration revision, when applicable.
    pub configuration_revision: Option<u64>,
    /// Trace correlation ID.
    pub trace_id: Option<String>,
    /// Event timestamp.
    pub at_millis: u64,
    /// Redacted bounded metadata envelope.
    pub metadata: BTreeMap<String, String>,
}

impl AuditEvent {
    /// Creates and redacts one event at the trust boundary.
    ///
    /// # Errors
    ///
    /// Rejects empty, control-character, oversized, or excessive metadata.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: RegistryScope,
        actor: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
        outcome: AuditOutcome,
        policy_revision: Option<u64>,
        configuration_revision: Option<u64>,
        trace_id: Option<String>,
        at_millis: u64,
        metadata: &BTreeMap<String, String>,
    ) -> Result<Self, AgentMeshError> {
        let event = Self {
            id: Uuid::new_v4(),
            scope,
            actor: actor.into(),
            action: action.into(),
            target: target.into(),
            outcome,
            policy_revision,
            configuration_revision,
            trace_id,
            at_millis,
            metadata: redact_metadata(metadata),
        };
        for value in [&event.actor, &event.action, &event.target] {
            validate_text(value, 1_024)?;
        }
        if event.metadata.len() > MAX_AUDIT_METADATA {
            return Err(configuration(
                "The audit metadata envelope has too many fields.",
            ));
        }
        for (key, value) in &event.metadata {
            validate_text(key, 128)?;
            validate_text(value, 2_048)?;
        }
        if let Some(trace_id) = &event.trace_id {
            validate_text(trace_id, 128)?;
        }
        Ok(event)
    }
}

/// Whether outbox saturation blocks the protected operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryPolicy {
    /// High-risk operations fail when durability cannot be accepted.
    Required,
    /// Lower-risk operations continue while incrementing a dropped counter.
    BestEffort,
}

/// One hash-chained outbox entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// Monotonic local sequence.
    pub sequence: u64,
    /// Durable event.
    pub event: AuditEvent,
    /// Previous record digest.
    pub previous_hash: String,
    /// This record digest.
    pub hash: String,
    /// Export attempts.
    pub attempts: u32,
    /// Last stable exporter error code.
    pub last_error: Option<String>,
}

#[derive(Default)]
struct State {
    records: VecDeque<AuditRecord>,
    next_sequence: u64,
    last_hash: String,
    dropped: u64,
}

/// Backend-independent audit sink contract.
pub trait AuditSink: Send + Sync {
    /// Records one event according to its durability policy.
    ///
    /// # Errors
    ///
    /// Required events fail when durability is unavailable.
    fn record(
        &self,
        event: AuditEvent,
        policy: DeliveryPolicy,
    ) -> Result<Option<u64>, AgentMeshError>;
}

/// Thread-safe local durable-outbox model.
pub struct InMemoryAuditOutbox {
    capacity: usize,
    state: Mutex<State>,
}

impl InMemoryAuditOutbox {
    /// Creates an outbox with an explicit hard bound.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive capacity.
    pub fn new(capacity: usize) -> Result<Self, AgentMeshError> {
        if capacity == 0 || capacity > MAX_AUDIT_RECORDS {
            return Err(configuration("Audit outbox capacity is zero or unbounded."));
        }
        Ok(Self {
            capacity,
            state: Mutex::new(State {
                last_hash: "0".repeat(64),
                ..State::default()
            }),
        })
    }

    /// Returns up to `limit` pending records without removing them.
    pub fn pending(&self, limit: usize) -> Vec<AuditRecord> {
        lock(&self.state)
            .records
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Marks one exporter attempt using only a stable non-sensitive error code.
    ///
    /// # Errors
    ///
    /// Returns not found for an unknown/acknowledged sequence.
    pub fn mark_attempt(
        &self,
        sequence: u64,
        error_code: Option<&str>,
    ) -> Result<(), AgentMeshError> {
        let mut state = lock(&self.state);
        let record = state
            .records
            .iter_mut()
            .find(|record| record.sequence == sequence)
            .ok_or_else(|| {
                AgentMeshError::new(
                    ErrorCode::StorageUnavailable,
                    "The audit record is unavailable.",
                )
            })?;
        record.attempts = record.attempts.saturating_add(1);
        record.last_error = error_code.map(str::to_owned);
        if let Some(code) = &record.last_error {
            validate_text(code, 128)?;
        }
        Ok(())
    }

    /// Acknowledges a contiguous exported prefix.
    pub fn acknowledge_through(&self, sequence: u64) -> usize {
        let mut state = lock(&self.state);
        let mut removed = 0;
        while state
            .records
            .front()
            .is_some_and(|record| record.sequence <= sequence)
        {
            state.records.pop_front();
            removed += 1;
        }
        removed
    }

    /// Number of best-effort events dropped at capacity.
    pub fn dropped(&self) -> u64 {
        lock(&self.state).dropped
    }

    /// Verifies record digests and internal chain links.
    ///
    /// # Errors
    ///
    /// Returns a storage error when tampering or corruption is detected.
    pub fn verify(records: &[AuditRecord]) -> Result<(), AgentMeshError> {
        for (index, record) in records.iter().enumerate() {
            if index > 0 && record.previous_hash != records[index - 1].hash {
                return Err(corrupt());
            }
            if record.hash != record_hash(record.sequence, &record.previous_hash, &record.event)? {
                return Err(corrupt());
            }
        }
        Ok(())
    }
}

impl AuditSink for InMemoryAuditOutbox {
    fn record(
        &self,
        event: AuditEvent,
        policy: DeliveryPolicy,
    ) -> Result<Option<u64>, AgentMeshError> {
        let mut state = lock(&self.state);
        if state.records.len() >= self.capacity {
            if policy == DeliveryPolicy::Required {
                return Err(AgentMeshError::new(
                    ErrorCode::StorageUnavailable,
                    "Required audit durability is unavailable.",
                ));
            }
            state.dropped = state.dropped.saturating_add(1);
            return Ok(None);
        }
        state.next_sequence = state.next_sequence.checked_add(1).ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The audit sequence is exhausted.",
            )
        })?;
        let sequence = state.next_sequence;
        let previous_hash = state.last_hash.clone();
        let hash = record_hash(sequence, &previous_hash, &event)?;
        state.last_hash.clone_from(&hash);
        state.records.push_back(AuditRecord {
            sequence,
            event,
            previous_hash,
            hash,
            attempts: 0,
            last_error: None,
        });
        Ok(Some(sequence))
    }
}

fn record_hash(
    sequence: u64,
    previous_hash: &str,
    event: &AuditEvent,
) -> Result<String, AgentMeshError> {
    let encoded = serde_json::to_vec(&(sequence, previous_hash, event)).map_err(|_| corrupt())?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}
fn validate_text(value: &str, max: usize) -> Result<(), AgentMeshError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(configuration("An audit value is invalid or unbounded."));
    }
    Ok(())
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn configuration(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
fn corrupt() -> AgentMeshError {
    AgentMeshError::new(ErrorCode::StorageUnavailable, "The audit chain is invalid.")
}
