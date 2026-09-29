//! In-memory request accounting for live monitoring.
//!
//! Every served `/mcp` request records one observation: normalized method,
//! optional routing detail (tool name or resource URI, never bodies or
//! credentials), upstream attribution, HTTP status, and latency. Snapshots
//! are served as JSON from `GET /metrics`. Cardinality is bounded: unknown
//! vendor methods collapse to `other` and only the most recent events are
//! retained.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Instant,
};

use agentmesh_protocol::{JsonRpcMessage, McpMethod};
use serde::Serialize;

/// Maximum recent events retained per gateway instance.
const RECENT_LIMIT: usize = 32;
/// Maximum routing detail length kept per event.
const DETAIL_LIMIT: usize = 128;

/// Aggregated counters for one method or upstream label.
#[derive(Debug, Default, Clone)]
struct Counters {
    requests: u64,
    errors: u64,
    total_latency_ms: u64,
    max_latency_ms: u64,
}

/// One retained recent request.
#[derive(Debug, Clone, Serialize)]
struct RecentEvent {
    /// Seconds since gateway start.
    age_secs: u64,
    method: String,
    /// Tool name or resource URI, when the method carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    upstream: String,
    status: u16,
    latency_ms: u64,
    /// Policy decision label (`allow`, `deny:rule0`, …), when policies apply.
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct StatsData {
    methods: HashMap<String, Counters>,
    upstreams: HashMap<String, Counters>,
    recent: VecDeque<RecentEvent>,
}

/// Shared, lock-guarded request accounting.
#[derive(Debug, Clone)]
pub struct SharedStats {
    started: Instant,
    inner: Arc<Mutex<StatsData>>,
}

impl SharedStats {
    /// Creates empty accounting starting now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            inner: Arc::new(Mutex::new(StatsData::default())),
        }
    }

    /// Records one served request. Lock poison degrades to a dropped sample.
    pub fn record(
        &self,
        method: &str,
        detail: Option<String>,
        upstream: &str,
        status: u16,
        latency_ms: u64,
    ) {
        self.record_with_policy(method, detail, upstream, status, latency_ms, None);
    }

    /// Records one served request with its policy decision label.
    /// Lock poison degrades to a dropped sample.
    pub fn record_with_policy(
        &self,
        method: &str,
        detail: Option<String>,
        upstream: &str,
        status: u16,
        latency_ms: u64,
        policy: Option<String>,
    ) {
        let Ok(mut data) = self.inner.lock() else {
            return;
        };
        let failed = !(200..300).contains(&status);
        add_sample(
            data.methods.entry(method.to_string()).or_default(),
            failed,
            latency_ms,
        );
        add_sample(
            data.upstreams.entry(upstream.to_string()).or_default(),
            failed,
            latency_ms,
        );
        if data.recent.len() >= RECENT_LIMIT {
            data.recent.pop_front();
        }
        data.recent.push_back(RecentEvent {
            age_secs: self.started.elapsed().as_secs(),
            method: method.to_string(),
            detail: detail.map(|value| value.chars().take(DETAIL_LIMIT).collect()),
            upstream: upstream.to_string(),
            status,
            latency_ms,
            policy,
        });
    }

    /// Returns a serializable point-in-time snapshot.
    #[must_use]
    pub fn snapshot(&self) -> StatsSnapshot {
        let data: StatsData = self.inner.lock().as_deref().cloned().unwrap_or_default();
        StatsSnapshot {
            uptime_secs: self.started.elapsed().as_secs(),
            requests: data
                .methods
                .values()
                .map(|counters| counters.requests)
                .sum(),
            errors: data.methods.values().map(|counters| counters.errors).sum(),
            methods: data
                .methods
                .iter()
                .map(|(name, counters)| (name.clone(), MethodSnapshot::from(counters)))
                .collect(),
            upstreams: data
                .upstreams
                .iter()
                .map(|(name, counters)| (name.clone(), MethodSnapshot::from(counters)))
                .collect(),
            recent: data.recent.into_iter().collect(),
        }
    }
}

impl Default for SharedStats {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable per-label counters with averaged latency.
#[derive(Debug, Clone, Serialize)]
struct MethodSnapshot {
    requests: u64,
    errors: u64,
    avg_latency_ms: u64,
    max_latency_ms: u64,
}

impl From<&Counters> for MethodSnapshot {
    fn from(counters: &Counters) -> Self {
        Self {
            requests: counters.requests,
            errors: counters.errors,
            avg_latency_ms: if counters.requests == 0 {
                0
            } else {
                counters.total_latency_ms / counters.requests
            },
            max_latency_ms: counters.max_latency_ms,
        }
    }
}

/// Serializable gateway metrics snapshot served from `GET /metrics`.
#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    uptime_secs: u64,
    requests: u64,
    errors: u64,
    methods: HashMap<String, MethodSnapshot>,
    upstreams: HashMap<String, MethodSnapshot>,
    recent: Vec<RecentEvent>,
}

/// Adds one latency sample to aggregated counters.
fn add_sample(counters: &mut Counters, failed: bool, latency_ms: u64) {
    counters.requests = counters.requests.saturating_add(1);
    if failed {
        counters.errors = counters.errors.saturating_add(1);
    }
    counters.total_latency_ms = counters.total_latency_ms.saturating_add(latency_ms);
    counters.max_latency_ms = counters.max_latency_ms.max(latency_ms);
}

/// Normalizes a message to a bounded method label.
#[must_use]
pub fn method_label(message: &JsonRpcMessage) -> String {
    match message {
        JsonRpcMessage::Request(request) => match &request.method {
            McpMethod::Other(_) => "other".to_string(),
            method => method.as_str().to_string(),
        },
        JsonRpcMessage::Notification(_) => "notification".to_string(),
        JsonRpcMessage::Response(_) => "response".to_string(),
    }
}

/// Extracts the routing detail (tool name or resource URI) from a request.
#[must_use]
pub fn routing_detail(message: &JsonRpcMessage) -> Option<String> {
    let JsonRpcMessage::Request(request) = message else {
        return None;
    };
    let key = match request.method {
        McpMethod::ToolsCall | McpMethod::PromptsGet => "name",
        McpMethod::ResourcesRead
        | McpMethod::ResourcesSubscribe
        | McpMethod::ResourcesUnsubscribe => "uri",
        _ => return None,
    };
    request
        .params
        .as_ref()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_aggregates_and_bounds() {
        let stats = SharedStats::new();
        for index in 0..40u64 {
            stats.record("tools/call", Some(format!("tool-{index}")), "0", 200, index);
        }
        stats.record("tools/call", None, "0", 500, 10);
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.requests, 41);
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.recent.len(), RECENT_LIMIT);
        let methods = snapshot.methods.get("tools/call").expect("method row");
        assert_eq!(methods.requests, 41);
        assert_eq!(methods.max_latency_ms, 39);
        assert_eq!(
            snapshot.upstreams.get("0").expect("upstream row").requests,
            41
        );
    }

    #[test]
    fn labels_normalize_vendor_methods() {
        let unknown = JsonRpcMessage::Request(agentmesh_protocol::JsonRpcRequest::new(
            agentmesh_protocol::RequestId::Integer(1),
            McpMethod::from_wire("vendor/custom"),
            None,
        ));
        assert_eq!(method_label(&unknown), "other");
        assert_eq!(routing_detail(&unknown), None);
    }
}
