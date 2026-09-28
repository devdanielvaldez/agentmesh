//! Thread-safe endpoint selection with RAII request accounting.

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::{Endpoint, EndpointId, EndpointLifecycle};
use serde::{Deserialize, Serialize};

/// Maximum candidates held by one pool.
pub const MAX_POOL_ENDPOINTS: usize = 10_000;

/// Supported endpoint selection strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Stable rotating selection.
    RoundRobin,
    /// Deterministic pseudorandom selection from a caller-provided seed.
    Random,
    /// Selects the endpoint with the fewest active requests.
    LeastActive,
    /// Honors endpoint weights over a deterministic rotation.
    Weighted,
    /// Preserves affinity for a stable key.
    ConsistentHash,
    /// Favors low observed latency while accounting for errors and load.
    EwmaLeastLatency,
    /// Uses the same bounded signals as EWMA and endpoint weights.
    Adaptive,
}

/// Result signal used to update endpoint observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// Request latency in microseconds.
    pub latency_micros: u64,
    /// Whether the request completed successfully.
    pub success: bool,
}

#[derive(Debug, Clone, Copy)]
struct Stats {
    ewma_micros: u64,
    requests: u64,
    failures: u64,
}

struct Entry {
    endpoint: Endpoint,
    active: AtomicUsize,
    stats: Mutex<Stats>,
}

/// Immutable counters safe to export as bounded-cardinality metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointMetrics {
    /// Endpoint identifier.
    pub endpoint_id: EndpointId,
    /// Currently active requests.
    pub active: usize,
    /// Completed observed requests.
    pub requests: u64,
    /// Failed observed requests.
    pub failures: u64,
    /// Exponentially weighted latency in microseconds.
    pub ewma_micros: u64,
}

/// A bounded endpoint pool reusable across concurrent requests.
pub struct EndpointPool {
    entries: Vec<Arc<Entry>>,
    cursor: AtomicU64,
}

impl EndpointPool {
    /// Builds a validated pool from administratively active endpoints.
    ///
    /// # Errors
    ///
    /// Rejects empty, duplicate, unbounded, disabled, or zero-weight pools.
    pub fn new(endpoints: Vec<Endpoint>) -> Result<Self, AgentMeshError> {
        if endpoints.is_empty() || endpoints.len() > MAX_POOL_ENDPOINTS {
            return Err(invalid(
                "The endpoint pool is empty or exceeds its size limit.",
            ));
        }
        let mut identifiers = std::collections::BTreeSet::new();
        let mut entries = Vec::with_capacity(endpoints.len());
        for endpoint in endpoints {
            if endpoint.lifecycle != EndpointLifecycle::Active || endpoint.weight == 0 {
                return Err(invalid(
                    "Endpoint pools may contain only active, weighted endpoints.",
                ));
            }
            if !identifiers.insert(endpoint.id) {
                return Err(invalid("Endpoint pool identifiers must be unique."));
            }
            entries.push(Arc::new(Entry {
                endpoint,
                active: AtomicUsize::new(0),
                stats: Mutex::new(Stats {
                    ewma_micros: 0,
                    requests: 0,
                    failures: 0,
                }),
            }));
        }
        Ok(Self {
            entries,
            cursor: AtomicU64::new(0),
        })
    }

    /// Selects an endpoint and increments its active request count.
    ///
    /// The counter is released even when the returned lease is dropped during
    /// cancellation or unwinding.
    ///
    /// # Errors
    ///
    /// Consistent hashing requires a non-empty affinity key.
    pub fn select(
        &self,
        strategy: Strategy,
        affinity_key: Option<&[u8]>,
    ) -> Result<EndpointLease, AgentMeshError> {
        let index = match strategy {
            Strategy::RoundRobin => self.rotating_index(self.entries.len()),
            Strategy::Random => {
                let seed = self
                    .cursor
                    .fetch_add(1, Ordering::Relaxed)
                    .wrapping_add(0x9E37_79B9_7F4A_7C15);
                usize_from_u64(xorshift(seed)) % self.entries.len()
            }
            Strategy::LeastActive => self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| (entry.active.load(Ordering::Relaxed), entry.endpoint.id))
                .map_or(0, |(index, _)| index),
            Strategy::Weighted => self.weighted_index(None),
            Strategy::ConsistentHash => {
                let key = affinity_key.filter(|key| !key.is_empty()).ok_or_else(|| {
                    invalid("Consistent hashing requires a non-empty affinity key.")
                })?;
                self.weighted_index(Some(stable_hash(key)))
            }
            Strategy::EwmaLeastLatency => self.scored_index(false),
            Strategy::Adaptive => self.scored_index(true),
        };
        let entry = Arc::clone(&self.entries[index]);
        entry.active.fetch_add(1, Ordering::AcqRel);
        Ok(EndpointLease {
            entry,
            observation: None,
        })
    }

    /// Returns a stable endpoint-ID ordered metrics snapshot.
    pub fn metrics(&self) -> Vec<EndpointMetrics> {
        let mut result = self
            .entries
            .iter()
            .map(|entry| {
                let stats = lock(&entry.stats);
                EndpointMetrics {
                    endpoint_id: entry.endpoint.id,
                    active: entry.active.load(Ordering::Acquire),
                    requests: stats.requests,
                    failures: stats.failures,
                    ewma_micros: stats.ewma_micros,
                }
            })
            .collect::<Vec<_>>();
        result.sort_by_key(|metrics| metrics.endpoint_id);
        result
    }

    fn rotating_index(&self, length: usize) -> usize {
        usize_from_u64(self.cursor.fetch_add(1, Ordering::Relaxed)) % length
    }

    fn weighted_index(&self, supplied_ticket: Option<u64>) -> usize {
        let total = self.entries.iter().fold(0_u64, |sum, entry| {
            sum.saturating_add(u64::from(entry.endpoint.weight))
        });
        let ticket =
            supplied_ticket.unwrap_or_else(|| self.cursor.fetch_add(1, Ordering::Relaxed)) % total;
        let mut boundary = 0_u64;
        for (index, entry) in self.entries.iter().enumerate() {
            boundary = boundary.saturating_add(u64::from(entry.endpoint.weight));
            if ticket < boundary {
                return index;
            }
        }
        self.entries.len() - 1
    }

    fn scored_index(&self, use_weight: bool) -> usize {
        self.entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| {
                let stats = lock(&entry.stats);
                let latency = stats.ewma_micros.max(1);
                let error_penalty = if stats.requests == 0 {
                    0
                } else {
                    stats.failures.saturating_mul(latency).saturating_mul(4) / stats.requests
                };
                let load_penalty = u64::try_from(entry.active.load(Ordering::Relaxed))
                    .unwrap_or(u64::MAX)
                    .saturating_mul(latency);
                let score = latency
                    .saturating_add(error_penalty)
                    .saturating_add(load_penalty);
                if use_weight {
                    score / u64::from(entry.endpoint.weight.max(1))
                } else {
                    score
                }
            })
            .map_or(0, |(index, _)| index)
    }
}

/// Active request lease that cannot leak pool accounting.
pub struct EndpointLease {
    entry: Arc<Entry>,
    observation: Option<Observation>,
}

impl EndpointLease {
    /// Selected endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.entry.endpoint
    }

    /// Supplies the completion observation recorded when the lease is dropped.
    pub const fn observe(&mut self, observation: Observation) {
        self.observation = Some(observation);
    }
}

impl Drop for EndpointLease {
    fn drop(&mut self) {
        self.entry.active.fetch_sub(1, Ordering::AcqRel);
        if let Some(observation) = self.observation {
            let mut stats = lock(&self.entry.stats);
            stats.requests = stats.requests.saturating_add(1);
            stats.failures = stats
                .failures
                .saturating_add(u64::from(!observation.success));
            stats.ewma_micros = if stats.ewma_micros == 0 {
                observation.latency_micros
            } else {
                stats
                    .ewma_micros
                    .saturating_mul(7)
                    .saturating_add(observation.latency_micros)
                    / 8
            };
        }
    }
}

fn stable_hash(value: &[u8]) -> u64 {
    value.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn xorshift(mut value: u64) -> u64 {
    value ^= value << 13;
    value ^= value >> 7;
    value ^ (value << 17)
}

fn usize_from_u64(value: u64) -> usize {
    usize::try_from(value)
        .unwrap_or_else(|_| usize::try_from(value & u64::from(u32::MAX)).unwrap_or(0))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn invalid(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
