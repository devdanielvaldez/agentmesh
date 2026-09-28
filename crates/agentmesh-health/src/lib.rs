//! Bounded active/passive health observations and deterministic state transitions.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Mutex, MutexGuard},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::EndpointId;
use serde::{Deserialize, Serialize};

/// Maximum rolling observations retained per endpoint.
pub const MAX_HEALTH_WINDOW: usize = 4_096;
/// Maximum transition events retained until a consumer drains them.
pub const MAX_HEALTH_EVENTS: usize = 10_000;

/// Runtime endpoint condition, separate from administrative lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// No observation has arrived yet.
    Unknown,
    /// Endpoint accepts normal traffic.
    Healthy,
    /// Endpoint accepts reduced traffic.
    Degraded,
    /// Endpoint is excluded after operational failures.
    Unhealthy,
    /// Endpoint is proving recovery before full traffic.
    Recovering,
    /// Existing traffic may finish, but no new traffic is assigned.
    Draining,
    /// Administratively disabled.
    Disabled,
    /// Security quarantine, distinct from operational failure.
    Quarantined,
}

impl HealthState {
    /// Whether this state may receive new traffic.
    pub const fn routable(self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded | Self::Recovering)
    }
}

/// Source of a health signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    /// Scheduled protocol-aware probe.
    ActiveProbe,
    /// Real request result.
    PassiveRequest,
}

/// One bounded health signal using caller-supplied monotonic milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthObservation {
    /// Monotonic observation time.
    pub at_millis: u64,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Observed duration.
    pub latency_millis: u64,
    /// Signal origin.
    pub source: ObservationSource,
}

/// Validated health transition and scheduling settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthConfig {
    /// Rolling sample count.
    pub window_size: usize,
    /// Consecutive failures required to eject an endpoint.
    pub unhealthy_after_failures: usize,
    /// Failure ratio in basis points that marks degradation.
    pub degraded_failure_basis_points: u16,
    /// EWMA latency threshold for degradation.
    pub degraded_latency_millis: u64,
    /// Consecutive successes required after ejection.
    pub recovery_successes: usize,
    /// Base active probe interval.
    pub probe_interval_millis: u64,
    /// Maximum deterministic scheduling jitter.
    pub probe_jitter_millis: u64,
    /// Age after which the current signal is stale.
    pub stale_after_millis: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            window_size: 32,
            unhealthy_after_failures: 3,
            degraded_failure_basis_points: 2_000,
            degraded_latency_millis: 2_000,
            recovery_successes: 2,
            probe_interval_millis: 10_000,
            probe_jitter_millis: 2_000,
            stale_after_millis: 30_000,
        }
    }
}

impl HealthConfig {
    /// Validates bounded windows, ratios, and timing values.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for unsafe or contradictory values.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if self.window_size == 0
            || self.window_size > MAX_HEALTH_WINDOW
            || self.unhealthy_after_failures == 0
            || self.unhealthy_after_failures > self.window_size
            || self.recovery_successes == 0
            || self.recovery_successes > self.window_size
            || self.degraded_failure_basis_points > 10_000
            || self.degraded_latency_millis == 0
            || self.probe_interval_millis == 0
            || self.stale_after_millis < self.probe_interval_millis
        {
            return Err(invalid(
                "Health configuration values are invalid or unbounded.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Record {
    state: HealthState,
    samples: VecDeque<HealthObservation>,
    consecutive_failures: usize,
    consecutive_successes: usize,
    ewma_latency_millis: u64,
    last_observed_millis: Option<u64>,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            state: HealthState::Unknown,
            samples: VecDeque::new(),
            consecutive_failures: 0,
            consecutive_successes: 0,
            ewma_latency_millis: 0,
            last_observed_millis: None,
        }
    }
}

/// Sanitized endpoint health snapshot suitable for routing and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthSnapshot {
    /// Endpoint identifier.
    pub endpoint_id: EndpointId,
    /// Effective health state.
    pub state: HealthState,
    /// Samples currently retained.
    pub samples: usize,
    /// Rolling failures currently retained.
    pub failures: usize,
    /// Current EWMA latency.
    pub ewma_latency_millis: u64,
    /// Last observation time.
    pub last_observed_millis: Option<u64>,
}

/// A state transition event for telemetry/audit publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthEvent {
    /// Endpoint whose state changed.
    pub endpoint_id: EndpointId,
    /// Previous state.
    pub previous: HealthState,
    /// New state.
    pub current: HealthState,
    /// Monotonic transition time.
    pub at_millis: u64,
}

/// Thread-safe health table with bounded histories and transition events.
pub struct HealthManager {
    config: HealthConfig,
    records: Mutex<BTreeMap<EndpointId, Record>>,
    events: Mutex<VecDeque<HealthEvent>>,
}

impl HealthManager {
    /// Creates a manager after validating operational defaults.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for invalid settings.
    pub fn new(config: HealthConfig) -> Result<Self, AgentMeshError> {
        config.validate()?;
        Ok(Self {
            config,
            records: Mutex::new(BTreeMap::new()),
            events: Mutex::new(VecDeque::new()),
        })
    }

    /// Records an active or passive signal and returns the new state.
    pub fn record(&self, endpoint_id: EndpointId, observation: HealthObservation) -> HealthState {
        let mut records = lock(&self.records);
        let record = records.entry(endpoint_id).or_default();
        if record
            .last_observed_millis
            .is_some_and(|last| observation.at_millis < last)
        {
            return record.state;
        }
        if matches!(
            record.state,
            HealthState::Draining | HealthState::Disabled | HealthState::Quarantined
        ) {
            return record.state;
        }
        if record.samples.len() == self.config.window_size {
            record.samples.pop_front();
        }
        record.samples.push_back(observation);
        record.last_observed_millis = Some(observation.at_millis);
        record.ewma_latency_millis = if record.ewma_latency_millis == 0 {
            observation.latency_millis
        } else {
            record
                .ewma_latency_millis
                .saturating_mul(7)
                .saturating_add(observation.latency_millis)
                / 8
        };
        if observation.success {
            record.consecutive_successes = record.consecutive_successes.saturating_add(1);
            record.consecutive_failures = 0;
        } else {
            record.consecutive_failures = record.consecutive_failures.saturating_add(1);
            record.consecutive_successes = 0;
        }
        let previous = record.state;
        record.state = next_state(record, &self.config);
        let current = record.state;
        drop(records);
        self.publish(endpoint_id, previous, current, observation.at_millis);
        current
    }

    /// Applies an administrative state that observations cannot override.
    ///
    /// # Errors
    ///
    /// Rejects operational states, which must be produced from observations.
    pub fn set_administrative(
        &self,
        endpoint_id: EndpointId,
        state: HealthState,
        at_millis: u64,
    ) -> Result<(), AgentMeshError> {
        if !matches!(
            state,
            HealthState::Draining
                | HealthState::Disabled
                | HealthState::Quarantined
                | HealthState::Unknown
        ) {
            return Err(invalid(
                "Only administrative or unknown states may be set directly.",
            ));
        }
        let mut records = lock(&self.records);
        let record = records.entry(endpoint_id).or_default();
        let previous = record.state;
        record.state = state;
        if state == HealthState::Unknown {
            record.samples.clear();
            record.consecutive_failures = 0;
            record.consecutive_successes = 0;
            record.last_observed_millis = None;
        }
        drop(records);
        self.publish(endpoint_id, previous, state, at_millis);
        Ok(())
    }

    /// Returns the current snapshot, treating stale operational data as unknown.
    pub fn snapshot(&self, endpoint_id: EndpointId, now_millis: u64) -> HealthSnapshot {
        let records = lock(&self.records);
        let record = records.get(&endpoint_id).cloned().unwrap_or_default();
        let stale = record
            .last_observed_millis
            .is_some_and(|last| now_millis.saturating_sub(last) > self.config.stale_after_millis);
        let effective_state = if stale
            && matches!(
                record.state,
                HealthState::Healthy | HealthState::Degraded | HealthState::Recovering
            ) {
            HealthState::Unknown
        } else {
            record.state
        };
        HealthSnapshot {
            endpoint_id,
            state: effective_state,
            samples: record.samples.len(),
            failures: record
                .samples
                .iter()
                .filter(|sample| !sample.success)
                .count(),
            ewma_latency_millis: record.ewma_latency_millis,
            last_observed_millis: record.last_observed_millis,
        }
    }

    /// Computes the next probe time with stable per-endpoint jitter.
    pub fn next_probe_millis(&self, endpoint_id: EndpointId, now_millis: u64) -> u64 {
        let jitter = if self.config.probe_jitter_millis == 0 {
            0
        } else {
            endpoint_id
                .as_uuid()
                .as_bytes()
                .iter()
                .fold(0_u64, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(u64::from(*byte))
                })
                % (self.config.probe_jitter_millis + 1)
        };
        now_millis
            .saturating_add(self.config.probe_interval_millis)
            .saturating_add(jitter)
    }

    /// Drains published transition events in occurrence order.
    pub fn drain_events(&self) -> Vec<HealthEvent> {
        lock(&self.events).drain(..).collect()
    }

    fn publish(
        &self,
        endpoint_id: EndpointId,
        previous: HealthState,
        current: HealthState,
        at_millis: u64,
    ) {
        if previous == current {
            return;
        }
        let mut events = lock(&self.events);
        if events.len() == MAX_HEALTH_EVENTS {
            events.pop_front();
        }
        events.push_back(HealthEvent {
            endpoint_id,
            previous,
            current,
            at_millis,
        });
    }
}

fn next_state(record: &Record, config: &HealthConfig) -> HealthState {
    if record.consecutive_failures >= config.unhealthy_after_failures {
        return HealthState::Unhealthy;
    }
    if record.state == HealthState::Unhealthy {
        return HealthState::Recovering;
    }
    if record.state == HealthState::Recovering
        && record.consecutive_successes < config.recovery_successes
    {
        return HealthState::Recovering;
    }
    let failures = record
        .samples
        .iter()
        .filter(|sample| !sample.success)
        .count();
    let failure_basis_points = failures.saturating_mul(10_000) / record.samples.len().max(1);
    if failure_basis_points >= usize::from(config.degraded_failure_basis_points)
        || record.ewma_latency_millis >= config.degraded_latency_millis
    {
        HealthState::Degraded
    } else {
        HealthState::Healthy
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalid(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
