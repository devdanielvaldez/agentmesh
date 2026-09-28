//! Bounded closed/open/half-open circuit state machines using monotonic time.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::EndpointId;
use serde::{Deserialize, Serialize};

/// Maximum samples retained by one circuit.
pub const MAX_CIRCUIT_WINDOW: usize = 4_096;
/// Maximum capability-name bytes in a circuit key.
pub const MAX_CAPABILITY_KEY_BYTES: usize = 1_024;

/// Identity of one independently protected upstream flow.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CircuitKey {
    /// Physical endpoint.
    pub endpoint_id: EndpointId,
    /// Optional capability-specific isolation.
    pub capability: Option<String>,
}

/// Validated circuit transition thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircuitConfig {
    /// Rolling result count.
    pub window_size: usize,
    /// Minimum samples before failure rate is evaluated.
    pub minimum_throughput: usize,
    /// Failure ratio in basis points that opens the circuit.
    pub failure_basis_points: u16,
    /// Time the circuit remains open before probing.
    pub open_millis: u64,
    /// Simultaneous trial requests in half-open state.
    pub half_open_permits: usize,
    /// Successful half-open trials required to close.
    pub close_after_successes: usize,
}

impl Default for CircuitConfig {
    fn default() -> Self {
        Self {
            window_size: 20,
            minimum_throughput: 10,
            failure_basis_points: 5_000,
            open_millis: 30_000,
            half_open_permits: 1,
            close_after_successes: 2,
        }
    }
}

impl CircuitConfig {
    /// Validates bounded windows and transition settings.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for unsafe values.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if self.window_size == 0
            || self.window_size > MAX_CIRCUIT_WINDOW
            || self.minimum_throughput == 0
            || self.minimum_throughput > self.window_size
            || self.failure_basis_points == 0
            || self.failure_basis_points > 10_000
            || self.open_millis == 0
            || self.half_open_permits == 0
            || self.close_after_successes == 0
        {
            return Err(invalid(
                "Circuit breaker configuration is invalid or unbounded.",
            ));
        }
        Ok(())
    }
}

/// Externally visible circuit state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// Requests flow and results are sampled.
    Closed,
    /// Requests fail fast until the cooldown elapses.
    Open,
    /// A bounded number of recovery trials may run.
    HalfOpen,
}

#[derive(Debug)]
struct Inner {
    state: CircuitState,
    samples: VecDeque<bool>,
    opened_at_millis: Option<u64>,
    half_open_active: usize,
    half_open_successes: usize,
    rejected: u64,
    transitions: u64,
}

/// Stable metrics snapshot without sensitive request data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitSnapshot {
    /// Current state.
    pub state: CircuitState,
    /// Samples retained in the rolling window.
    pub samples: usize,
    /// Failures retained in the rolling window.
    pub failures: usize,
    /// Currently active half-open trials.
    pub half_open_active: usize,
    /// Requests rejected while open or saturated.
    pub rejected: u64,
    /// Total state transitions.
    pub transitions: u64,
}

/// Thread-safe circuit for one endpoint/capability key.
pub struct CircuitBreaker {
    key: CircuitKey,
    config: CircuitConfig,
    inner: Mutex<Inner>,
}

impl CircuitBreaker {
    /// Creates a validated circuit.
    ///
    /// # Errors
    ///
    /// Rejects invalid settings and unbounded capability keys.
    pub fn new(key: CircuitKey, config: CircuitConfig) -> Result<Arc<Self>, AgentMeshError> {
        config.validate()?;
        if key.capability.as_ref().is_some_and(|name| {
            name.trim().is_empty()
                || name.len() > MAX_CAPABILITY_KEY_BYTES
                || name.chars().any(char::is_control)
        }) {
            return Err(invalid(
                "The circuit capability key is invalid or unbounded.",
            ));
        }
        Ok(Arc::new(Self {
            key,
            config,
            inner: Mutex::new(Inner {
                state: CircuitState::Closed,
                samples: VecDeque::new(),
                opened_at_millis: None,
                half_open_active: 0,
                half_open_successes: 0,
                rejected: 0,
                transitions: 0,
            }),
        }))
    }

    /// Circuit identity.
    pub fn key(&self) -> &CircuitKey {
        &self.key
    }

    /// Acquires permission for one request at a monotonic timestamp.
    ///
    /// # Errors
    ///
    /// Returns `CircuitOpen` when the cooldown or half-open limit blocks traffic.
    pub fn acquire(self: &Arc<Self>, now_millis: u64) -> Result<CircuitPermit, AgentMeshError> {
        let mut inner = lock(&self.inner);
        if inner.state == CircuitState::Open
            && inner
                .opened_at_millis
                .is_some_and(|opened| now_millis.saturating_sub(opened) >= self.config.open_millis)
        {
            transition(&mut inner, CircuitState::HalfOpen);
            inner.half_open_successes = 0;
        }
        match inner.state {
            CircuitState::Closed => {}
            CircuitState::Open => {
                inner.rejected = inner.rejected.saturating_add(1);
                return Err(open_error());
            }
            CircuitState::HalfOpen if inner.half_open_active >= self.config.half_open_permits => {
                inner.rejected = inner.rejected.saturating_add(1);
                return Err(open_error());
            }
            CircuitState::HalfOpen => {
                inner.half_open_active += 1;
            }
        }
        drop(inner);
        Ok(CircuitPermit {
            breaker: Arc::clone(self),
            completed: false,
        })
    }

    /// Returns current state and low-cardinality counters.
    pub fn snapshot(&self) -> CircuitSnapshot {
        let inner = lock(&self.inner);
        CircuitSnapshot {
            state: inner.state,
            samples: inner.samples.len(),
            failures: inner.samples.iter().filter(|success| !**success).count(),
            half_open_active: inner.half_open_active,
            rejected: inner.rejected,
            transitions: inner.transitions,
        }
    }

    fn complete(&self, success: bool, now_millis: u64) {
        let mut inner = lock(&self.inner);
        match inner.state {
            CircuitState::Closed => {
                if inner.samples.len() == self.config.window_size {
                    inner.samples.pop_front();
                }
                inner.samples.push_back(success);
                if inner.samples.len() >= self.config.minimum_throughput {
                    let failures = inner.samples.iter().filter(|item| !**item).count();
                    if failures.saturating_mul(10_000) / inner.samples.len()
                        >= usize::from(self.config.failure_basis_points)
                    {
                        transition(&mut inner, CircuitState::Open);
                        inner.opened_at_millis = Some(now_millis);
                    }
                }
            }
            CircuitState::HalfOpen => {
                inner.half_open_active = inner.half_open_active.saturating_sub(1);
                if success {
                    inner.half_open_successes = inner.half_open_successes.saturating_add(1);
                    if inner.half_open_successes >= self.config.close_after_successes {
                        transition(&mut inner, CircuitState::Closed);
                        inner.samples.clear();
                        inner.opened_at_millis = None;
                        inner.half_open_successes = 0;
                    }
                } else {
                    transition(&mut inner, CircuitState::Open);
                    inner.opened_at_millis = Some(now_millis);
                    inner.half_open_successes = 0;
                }
            }
            CircuitState::Open => {}
        }
    }

    fn abandon(&self) {
        let mut inner = lock(&self.inner);
        if inner.state == CircuitState::HalfOpen {
            inner.half_open_active = inner.half_open_active.saturating_sub(1);
        }
    }
}

/// In-flight circuit permission. Dropping it releases half-open capacity.
pub struct CircuitPermit {
    breaker: Arc<CircuitBreaker>,
    completed: bool,
}

impl CircuitPermit {
    /// Records a successful or failed upstream attempt.
    pub fn complete(mut self, success: bool, now_millis: u64) {
        self.breaker.complete(success, now_millis);
        self.completed = true;
    }
}

impl Drop for CircuitPermit {
    fn drop(&mut self) {
        if !self.completed {
            self.breaker.abandon();
        }
    }
}

fn transition(inner: &mut Inner, state: CircuitState) {
    if inner.state != state {
        inner.state = state;
        inner.transitions = inner.transitions.saturating_add(1);
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

fn open_error() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::CircuitOpen,
        "The upstream circuit is not accepting requests.",
    )
}
