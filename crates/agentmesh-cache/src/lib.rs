//! Bounded revision-aware caching for explicitly cacheable MCP data.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_registry::RegistryScope;
use serde::{Deserialize, Serialize};

/// Cacheable data families. Mutating tool calls are deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheKind {
    /// Normalized capability catalog.
    CapabilityCatalog,
    /// Endpoint or MCP discovery result.
    Discovery,
    /// Authorization/policy decision.
    PolicyDecision,
    /// Identity-provider JWKS document.
    Jwks,
    /// Explicitly cacheable resource read.
    ResourceRead,
}

/// Complete isolation key for cached data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey {
    /// Tenant boundary.
    pub scope: RegistryScope,
    /// Cached data family.
    pub kind: CacheKind,
    /// Identity/policy visibility scope or `public`.
    pub identity_scope: String,
    /// MCP protocol revision.
    pub protocol_version: String,
    /// Configuration/policy revision.
    pub configuration_revision: u64,
    /// Stable capability, URI, issuer, or discovery key.
    pub resource: String,
}

impl CacheKey {
    /// Validates every externally-derived key component.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, or control-character values.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        for value in [&self.identity_scope, &self.protocol_version, &self.resource] {
            if value.trim().is_empty() || value.len() > 2_048 || value.chars().any(char::is_control)
            {
                return Err(configuration(
                    "A cache key component is invalid or unbounded.",
                ));
            }
        }
        Ok(())
    }
}

/// Validated cache resource limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachePolicy {
    /// Maximum entries.
    pub max_entries: usize,
    /// Maximum cumulative payload bytes.
    pub max_bytes: usize,
    /// Maximum bytes in one payload.
    pub max_entry_bytes: usize,
    /// Maximum accepted TTL.
    pub max_ttl_millis: u64,
    /// Whether explicit resource-read responses may be cached.
    pub allow_resource_reads: bool,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_bytes: 64 * 1_024 * 1_024,
            max_entry_bytes: 1_024 * 1_024,
            max_ttl_millis: 300_000,
            allow_resource_reads: false,
        }
    }
}

impl CachePolicy {
    /// Validates internally consistent non-zero bounds.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for zero or contradictory limits.
    pub fn validate(&self) -> Result<(), AgentMeshError> {
        if self.max_entries == 0
            || self.max_bytes == 0
            || self.max_entry_bytes == 0
            || self.max_entry_bytes > self.max_bytes
            || self.max_ttl_millis == 0
        {
            return Err(configuration(
                "Cache policy limits are zero or inconsistent.",
            ));
        }
        Ok(())
    }
}

struct Entry {
    value: Arc<[u8]>,
    expires_at_millis: u64,
    last_access: u64,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<CacheKey, Entry>,
    bytes: usize,
    sequence: u64,
}

/// Shared response bytes with redacted formatting.
pub struct CacheLease(Arc<[u8]>);

impl CacheLease {
    /// Cached serialized payload.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for CacheLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CacheLease")
            .field(&"[REDACTED]")
            .finish()
    }
}

/// Cache counters suitable for bounded telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheMetrics {
    /// Cache hits.
    pub hits: u64,
    /// Cache misses, including expired entries.
    pub misses: u64,
    /// Capacity evictions.
    pub evictions: u64,
    /// Current entries.
    pub entries: usize,
    /// Current payload bytes.
    pub bytes: usize,
}

/// Thread-safe bounded standalone cache.
pub struct McpCache {
    policy: CachePolicy,
    state: Mutex<State>,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl McpCache {
    /// Creates an empty cache with validated bounds.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for invalid policy.
    pub fn new(policy: CachePolicy) -> Result<Self, AgentMeshError> {
        policy.validate()?;
        Ok(Self {
            policy,
            state: Mutex::new(State::default()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        })
    }

    /// Reads and touches a non-expired exact key.
    pub fn get(&self, key: &CacheKey, now_millis: u64) -> Option<CacheLease> {
        let mut state = lock(&self.state);
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        if state
            .entries
            .get(key)
            .is_some_and(|entry| entry.expires_at_millis <= now_millis)
        {
            if let Some(expired) = state.entries.remove(key) {
                state.bytes = state.bytes.saturating_sub(expired.value.len());
            }
        }
        if let Some(entry) = state.entries.get_mut(key) {
            entry.last_access = sequence;
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(CacheLease(Arc::clone(&entry.value)));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Stores explicitly cacheable bytes with a bounded TTL.
    ///
    /// # Errors
    ///
    /// Rejects invalid keys, resource reads without opt-in, empty/oversized values, and invalid TTLs.
    pub fn put(
        &self,
        key: CacheKey,
        value: Vec<u8>,
        ttl_millis: u64,
        now_millis: u64,
    ) -> Result<(), AgentMeshError> {
        key.validate()?;
        if key.kind == CacheKind::ResourceRead && !self.policy.allow_resource_reads {
            return Err(AgentMeshError::new(
                ErrorCode::PolicyDenied,
                "Resource response caching is not enabled.",
            ));
        }
        if value.is_empty() || value.len() > self.policy.max_entry_bytes {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The cache entry is empty or exceeds its size limit.",
            ));
        }
        if ttl_millis == 0 || ttl_millis > self.policy.max_ttl_millis {
            return Err(configuration("The cache TTL is zero or exceeds policy."));
        }
        let expires_at_millis = now_millis
            .checked_add(ttl_millis)
            .ok_or_else(|| configuration("The cache expiry overflows monotonic time."))?;
        let mut state = lock(&self.state);
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        if let Some(previous) = state.entries.remove(&key) {
            state.bytes = state.bytes.saturating_sub(previous.value.len());
        }
        while state.entries.len() >= self.policy.max_entries
            || state.bytes.saturating_add(value.len()) > self.policy.max_bytes
        {
            let Some(eviction_key) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = state.entries.remove(&eviction_key) {
                state.bytes = state.bytes.saturating_sub(entry.value.len());
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
        let value: Arc<[u8]> = value.into();
        state.bytes += value.len();
        state.entries.insert(
            key,
            Entry {
                value,
                expires_at_millis,
                last_access: sequence,
            },
        );
        Ok(())
    }

    /// Invalidates all entries for one tenant below a current revision.
    pub fn invalidate_before_revision(
        &self,
        scope: &RegistryScope,
        current_revision: u64,
    ) -> usize {
        self.retain(|key| &key.scope != scope || key.configuration_revision >= current_revision)
    }

    /// Invalidates every entry in one tenant.
    pub fn invalidate_scope(&self, scope: &RegistryScope) -> usize {
        self.retain(|key| &key.scope != scope)
    }

    /// Returns current low-cardinality metrics.
    pub fn metrics(&self) -> CacheMetrics {
        let state = lock(&self.state);
        CacheMetrics {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            entries: state.entries.len(),
            bytes: state.bytes,
        }
    }

    fn retain(&self, predicate: impl Fn(&CacheKey) -> bool) -> usize {
        let mut state = lock(&self.state);
        let before = state.entries.len();
        state.entries.retain(|key, _| predicate(key));
        state.bytes = state.entries.values().map(|entry| entry.value.len()).sum();
        before - state.entries.len()
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
