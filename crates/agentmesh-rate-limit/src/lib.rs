//! Atomic local rate, quota, and concurrency enforcement with scoped keys.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};

/// Maximum distinct counters retained by one local limiter.
pub const MAX_LIMIT_KEYS: usize = 1_000_000;

/// Supported enforcement dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitDimension {
    /// Whole tenant scope.
    Tenant,
    /// Authenticated principal.
    Principal,
    /// Validated source IP.
    Ip,
    /// Logical service.
    Service,
    /// Tool, resource, prompt, or task capability.
    Capability,
}

/// Revision-aware counter key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LimitKey {
    /// Tenant boundary.
    pub scope: RegistryScope,
    /// Counter dimension.
    pub dimension: LimitDimension,
    /// Normalized subject within the dimension.
    pub subject: String,
    /// Configuration revision, preventing stale counter reuse.
    pub policy_revision: u64,
}

impl LimitKey {
    /// Creates a validated scoped key.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, or control-character subjects.
    pub fn new(
        scope: RegistryScope,
        dimension: LimitDimension,
        subject: impl Into<String>,
        policy_revision: u64,
    ) -> Result<Self, AgentMeshError> {
        let subject = subject.into();
        if subject.trim().is_empty()
            || subject.len() > 1_024
            || subject.chars().any(char::is_control)
        {
            return Err(configuration(
                "The rate-limit subject is invalid or unbounded.",
            ));
        }
        Ok(Self {
            scope,
            dimension,
            subject,
            policy_revision,
        })
    }
}

/// Combined local limit policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitPolicy {
    /// Token bucket capacity.
    pub rate_capacity: u64,
    /// Tokens added per second.
    pub rate_refill_per_second: u64,
    /// Maximum simultaneous leases.
    pub concurrency: usize,
    /// Requests/cost units per quota window.
    pub quota: u64,
    /// Fixed quota window length.
    pub quota_window_millis: u64,
}

impl Default for LimitPolicy {
    fn default() -> Self {
        Self {
            rate_capacity: 100,
            rate_refill_per_second: 100,
            concurrency: 32,
            quota: 10_000,
            quota_window_millis: 86_400_000,
        }
    }
}

impl LimitPolicy {
    /// Validates all limits are non-zero and multiplication-safe.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for zero or overflowing values.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if self.rate_capacity == 0
            || self.rate_refill_per_second == 0
            || self.concurrency == 0
            || self.quota == 0
            || self.quota_window_millis == 0
            || self.rate_capacity.checked_mul(1_000).is_none()
        {
            return Err(configuration(
                "Rate, concurrency, and quota values must be positive and bounded.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Counter {
    tokens_milli: u64,
    last_refill_millis: u64,
    active: usize,
    quota_used: u64,
    quota_window_started_millis: u64,
    last_seen_millis: u64,
}

/// Observable counter state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitSnapshot {
    /// Approximate whole tokens available.
    pub available_tokens: u64,
    /// Current concurrent requests.
    pub active: usize,
    /// Cost consumed in the current quota window.
    pub quota_used: u64,
    /// Current quota window start.
    pub quota_window_started_millis: u64,
}

/// Thread-safe standalone limiter. Distributed implementations can preserve this contract.
pub struct InMemoryRateLimiter {
    max_keys: usize,
    counters: Mutex<BTreeMap<LimitKey, Counter>>,
}

impl InMemoryRateLimiter {
    /// Creates a limiter with an explicit cardinality bound.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive key capacity.
    pub fn new(max_keys: usize) -> Result<Arc<Self>, AgentMeshError> {
        if max_keys == 0 || max_keys > MAX_LIMIT_KEYS {
            return Err(configuration(
                "The rate-limit key capacity is invalid or unbounded.",
            ));
        }
        Ok(Arc::new(Self {
            max_keys,
            counters: Mutex::new(BTreeMap::new()),
        }))
    }

    /// Atomically enforces rate, quota, and concurrency and returns an RAII lease.
    ///
    /// # Errors
    ///
    /// Returns stable rate, quota, concurrency, configuration, or capacity errors.
    pub fn acquire(
        self: &Arc<Self>,
        key: &LimitKey,
        policy: &LimitPolicy,
        cost: u64,
        now_millis: u64,
    ) -> Result<LimitLease, AgentMeshError> {
        policy.validate()?;
        if cost == 0 || cost > policy.rate_capacity || cost > policy.quota {
            return Err(configuration(
                "The request cost is zero or exceeds configured capacity.",
            ));
        }
        let mut counters = lock(&self.counters);
        if !counters.contains_key(key) && counters.len() >= self.max_keys {
            return Err(AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The local limit counter store is at capacity.",
            ));
        }
        let counter = counters.entry(key.clone()).or_insert_with(|| Counter {
            tokens_milli: policy.rate_capacity * 1_000,
            last_refill_millis: now_millis,
            active: 0,
            quota_used: 0,
            quota_window_started_millis: now_millis,
            last_seen_millis: now_millis,
        });
        if now_millis.saturating_sub(counter.quota_window_started_millis)
            >= policy.quota_window_millis
        {
            counter.quota_used = 0;
            counter.quota_window_started_millis = now_millis;
        }
        let elapsed = now_millis.saturating_sub(counter.last_refill_millis);
        counter.tokens_milli = counter
            .tokens_milli
            .saturating_add(elapsed.saturating_mul(policy.rate_refill_per_second))
            .min(policy.rate_capacity * 1_000);
        counter.last_refill_millis = counter.last_refill_millis.max(now_millis);
        counter.last_seen_millis = counter.last_seen_millis.max(now_millis);
        let cost_milli = cost * 1_000;
        if counter.quota_used.saturating_add(cost) > policy.quota {
            return Err(AgentMeshError::new(
                ErrorCode::QuotaExceeded,
                "The usage quota is exhausted.",
            ));
        }
        if counter.tokens_milli < cost_milli {
            return Err(AgentMeshError::new(
                ErrorCode::RateLimited,
                "The request rate is exceeded.",
            ));
        }
        if counter.active >= policy.concurrency {
            return Err(AgentMeshError::new(
                ErrorCode::ConcurrencyLimited,
                "The concurrent request limit is exceeded.",
            ));
        }
        counter.tokens_milli -= cost_milli;
        counter.quota_used += cost;
        counter.active += 1;
        Ok(LimitLease {
            limiter: Arc::clone(self),
            key: key.clone(),
        })
    }

    /// Reads a counter snapshot if the key has been observed.
    pub fn snapshot(&self, key: &LimitKey) -> Option<LimitSnapshot> {
        lock(&self.counters).get(key).map(|counter| LimitSnapshot {
            available_tokens: counter.tokens_milli / 1_000,
            active: counter.active,
            quota_used: counter.quota_used,
            quota_window_started_millis: counter.quota_window_started_millis,
        })
    }

    /// Removes idle counters with no active leases, bounded by `limit`.
    pub fn purge_idle(&self, now_millis: u64, idle_millis: u64, limit: usize) -> usize {
        let mut counters = lock(&self.counters);
        let keys = counters
            .iter()
            .filter(|(_, counter)| {
                counter.active == 0
                    && now_millis.saturating_sub(counter.last_seen_millis) >= idle_millis
            })
            .take(limit)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let removed = keys.len();
        for key in keys {
            counters.remove(&key);
        }
        removed
    }

    fn release(&self, key: &LimitKey) {
        if let Some(counter) = lock(&self.counters).get_mut(key) {
            counter.active = counter.active.saturating_sub(1);
        }
    }
}

/// Active concurrency reservation released on every exit path.
pub struct LimitLease {
    limiter: Arc<InMemoryRateLimiter>,
    key: LimitKey,
}

impl Drop for LimitLease {
    fn drop(&mut self) {
        self.limiter.release(&self.key);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn configuration(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
