//! Bounded reliability primitives with no hidden I/O or background tasks.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use agentmesh_error::{AgentMeshError, ErrorCode, RetryClass};
use serde::{Deserialize, Serialize};

/// Hard upper bound on automated attempts, including the first attempt.
pub const MAX_RETRY_ATTEMPTS: u8 = 10;
/// Hard upper bound on items in an in-process queue.
pub const MAX_QUEUE_CAPACITY: usize = 100_000;

/// Whether retrying an operation can be safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idempotency {
    /// Read-only or protocol-defined safe operation.
    Safe,
    /// Mutating operation carrying a verified idempotency mechanism.
    Verified,
    /// Mutating operation that must never be retried automatically.
    Unsafe,
}

/// Validated retry and exponential-backoff policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Total attempts, including the initial call.
    pub max_attempts: u8,
    /// Initial backoff.
    pub base_delay_millis: u64,
    /// Backoff ceiling.
    pub max_delay_millis: u64,
    /// Maximum deterministic jitter in basis points.
    pub jitter_basis_points: u16,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_millis: 50,
            max_delay_millis: 2_000,
            jitter_basis_points: 2_000,
        }
    }
}

impl RetryPolicy {
    /// Validates bounded attempts, delays, and jitter.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for unsafe values.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if self.max_attempts == 0
            || self.max_attempts > MAX_RETRY_ATTEMPTS
            || self.base_delay_millis == 0
            || self.max_delay_millis < self.base_delay_millis
            || self.jitter_basis_points > 10_000
        {
            return Err(invalid("Retry policy values are invalid or unbounded."));
        }
        Ok(())
    }

    /// Returns whether another attempt is safe and allowed.
    pub fn should_retry(
        &self,
        completed_attempts: u8,
        error: &AgentMeshError,
        idempotency: Idempotency,
    ) -> bool {
        completed_attempts < self.max_attempts
            && idempotency != Idempotency::Unsafe
            && error.code().retry_class() == RetryClass::Backoff
    }

    /// Computes capped exponential delay with reproducible full jitter.
    pub fn delay(&self, completed_attempts: u8, seed: u64) -> Duration {
        let exponent = u32::from(completed_attempts.saturating_sub(1).min(62));
        let base = self
            .base_delay_millis
            .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
            .min(self.max_delay_millis);
        let jitter_max = base.saturating_mul(u64::from(self.jitter_basis_points)) / 10_000;
        let jitter = if jitter_max == 0 {
            0
        } else {
            mix(seed) % (jitter_max + 1)
        };
        Duration::from_millis(base.saturating_sub(jitter_max / 2).saturating_add(jitter))
    }
}

/// Per-request deadline budget using monotonic caller-supplied milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    started_millis: u64,
    expires_millis: u64,
}

impl Deadline {
    /// Creates a non-zero, overflow-safe total deadline.
    ///
    /// # Errors
    ///
    /// Rejects zero duration and monotonic timestamp overflow.
    pub fn new(started_millis: u64, total_millis: u64) -> Result<Self, AgentMeshError> {
        let expires_millis = started_millis
            .checked_add(total_millis)
            .filter(|_| total_millis > 0)
            .ok_or_else(|| invalid("The request deadline is zero or overflows monotonic time."))?;
        Ok(Self {
            started_millis,
            expires_millis,
        })
    }

    /// Initial monotonic timestamp.
    pub const fn started_millis(self) -> u64 {
        self.started_millis
    }

    /// Remaining duration, or zero once expired.
    pub fn remaining(self, now_millis: u64) -> Duration {
        Duration::from_millis(self.expires_millis.saturating_sub(now_millis))
    }

    /// Whether no execution time remains.
    pub const fn expired(self, now_millis: u64) -> bool {
        now_millis >= self.expires_millis
    }

    /// Caps one operation timeout by the total remaining budget.
    pub fn cap(self, requested: Duration, now_millis: u64) -> Duration {
        requested.min(self.remaining(now_millis))
    }
}

#[derive(Debug)]
struct BudgetState {
    tokens_milli: u64,
    last_refill_millis: u64,
}

/// Local token bucket limiting retry amplification.
pub struct RetryBudget {
    capacity: u64,
    refill_per_second: u64,
    state: Mutex<BudgetState>,
}

impl RetryBudget {
    /// Creates a full retry token bucket.
    ///
    /// # Errors
    ///
    /// Capacity and refill must both be non-zero.
    pub fn new(
        capacity: u64,
        refill_per_second: u64,
        now_millis: u64,
    ) -> Result<Self, AgentMeshError> {
        if capacity == 0 || refill_per_second == 0 {
            return Err(invalid(
                "Retry budget capacity and refill must be non-zero.",
            ));
        }
        Ok(Self {
            capacity,
            refill_per_second,
            state: Mutex::new(BudgetState {
                tokens_milli: capacity.saturating_mul(1_000),
                last_refill_millis: now_millis,
            }),
        })
    }

    /// Atomically consumes one retry token when available.
    pub fn try_consume(&self, now_millis: u64) -> bool {
        let mut state = lock(&self.state);
        let elapsed = now_millis.saturating_sub(state.last_refill_millis);
        let refill = elapsed.saturating_mul(self.refill_per_second);
        state.tokens_milli = state
            .tokens_milli
            .saturating_add(refill)
            .min(self.capacity.saturating_mul(1_000));
        state.last_refill_millis = state.last_refill_millis.max(now_millis);
        if state.tokens_milli < 1_000 {
            return false;
        }
        state.tokens_milli -= 1_000;
        true
    }
}

/// Low-cardinality priority classes for bounded queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Latency-sensitive control or interactive work.
    High,
    /// Normal request traffic.
    Normal,
    /// Deferrable background work.
    Background,
}

/// FIFO queue within each priority, always draining higher priority first.
pub struct BoundedPriorityQueue<T> {
    capacity: usize,
    queues: Mutex<[VecDeque<T>; 3]>,
}

impl<T> BoundedPriorityQueue<T> {
    /// Creates a queue with an explicit hard bound.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive capacity.
    pub fn new(capacity: usize) -> Result<Self, AgentMeshError> {
        if capacity == 0 || capacity > MAX_QUEUE_CAPACITY {
            return Err(invalid("Queue capacity is zero or exceeds its hard limit."));
        }
        Ok(Self {
            capacity,
            queues: Mutex::new(std::array::from_fn(|_| VecDeque::new())),
        })
    }

    /// Enqueues an item or returns it unchanged when backpressure is active.
    ///
    /// # Errors
    ///
    /// Returns the original item when the queue is at capacity.
    pub fn push(&self, priority: Priority, item: T) -> Result<(), T> {
        let mut queues = lock(&self.queues);
        if queues.iter().map(VecDeque::len).sum::<usize>() >= self.capacity {
            return Err(item);
        }
        queues[priority_index(priority)].push_back(item);
        Ok(())
    }

    /// Removes the oldest item from the highest non-empty priority.
    pub fn pop(&self) -> Option<T> {
        let mut queues = lock(&self.queues);
        queues.iter_mut().find_map(VecDeque::pop_front)
    }

    /// Current total queue depth.
    pub fn len(&self) -> usize {
        lock(&self.queues).iter().map(VecDeque::len).sum()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A concurrency bulkhead with fail-fast draining.
pub struct Bulkhead {
    limit: usize,
    active: AtomicUsize,
    draining: AtomicBool,
}

impl Bulkhead {
    /// Creates a bounded concurrency pool.
    ///
    /// # Errors
    ///
    /// Rejects a zero concurrency limit.
    pub fn new(limit: usize) -> Result<Arc<Self>, AgentMeshError> {
        if limit == 0 {
            return Err(invalid("Bulkhead concurrency must be greater than zero."));
        }
        Ok(Arc::new(Self {
            limit,
            active: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
        }))
    }

    /// Attempts to reserve one active slot.
    ///
    /// # Errors
    ///
    /// Returns an availability or concurrency error without waiting.
    pub fn try_acquire(self: &Arc<Self>) -> Result<BulkheadPermit, AgentMeshError> {
        if self.draining.load(Ordering::Acquire) {
            return Err(AgentMeshError::new(
                ErrorCode::UpstreamUnavailable,
                "The traffic pool is draining.",
            ));
        }
        let result = self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.limit).then_some(active + 1)
            });
        result.map_err(|_| {
            AgentMeshError::new(
                ErrorCode::ConcurrencyLimited,
                "The concurrency pool is full.",
            )
        })?;
        if self.draining.load(Ordering::Acquire) {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(AgentMeshError::new(
                ErrorCode::UpstreamUnavailable,
                "The traffic pool is draining.",
            ));
        }
        Ok(BulkheadPermit {
            bulkhead: Arc::clone(self),
        })
    }

    /// Stops new admissions while existing permits drain naturally.
    pub fn begin_drain(&self) {
        self.draining.store(true, Ordering::Release);
    }

    /// Re-enables admissions after an operator-controlled drain.
    pub fn resume(&self) {
        self.draining.store(false, Ordering::Release);
    }

    /// Current active requests.
    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// Whether shutdown or endpoint draining is active.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }
}

/// RAII concurrency reservation.
pub struct BulkheadPermit {
    bulkhead: Arc<Bulkhead>,
}

impl Drop for BulkheadPermit {
    fn drop(&mut self) {
        self.bulkhead.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn priority_index(priority: Priority) -> usize {
    match priority {
        Priority::High => 0,
        Priority::Normal => 1,
        Priority::Background => 2,
    }
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalid(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
