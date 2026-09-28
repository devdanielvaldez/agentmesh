//! Vendor-neutral trace propagation, cardinality-safe metrics, and structured events.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Mutex, MutexGuard},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum attributes attached to one metric/event.
pub const MAX_ATTRIBUTES: usize = 32;
/// Maximum metric series in the local recorder.
pub const MAX_METRIC_SERIES: usize = 100_000;
/// Maximum buffered structured events.
pub const MAX_TELEMETRY_EVENTS: usize = 10_000;

/// Validated W3C trace context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: String,
    parent_id: String,
    flags: u8,
}

impl TraceContext {
    /// Creates a root sampled trace.
    pub fn root() -> Self {
        let span_source = Uuid::new_v4().as_u128() & u128::from(u64::MAX);
        let span_id = u64::try_from(span_source).unwrap_or_default();
        Self {
            trace_id: format!("{:032x}", Uuid::new_v4().as_u128()),
            parent_id: format!("{span_id:016x}"),
            flags: 1,
        }
    }

    /// Parses a W3C `traceparent` value.
    ///
    /// # Errors
    ///
    /// Rejects unsupported versions, malformed hex, and all-zero IDs.
    pub fn parse(value: &str) -> Result<Self, AgentMeshError> {
        let parts = value.split('-').collect::<Vec<_>>();
        if parts.len() != 4
            || parts[0] != "00"
            || parts[1].len() != 32
            || parts[2].len() != 16
            || parts[3].len() != 2
            || parts[1].bytes().all(|byte| byte == b'0')
            || parts[2].bytes().all(|byte| byte == b'0')
            || !parts
                .iter()
                .all(|part| part.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(AgentMeshError::new(
                ErrorCode::InvalidRequest,
                "The trace context is invalid.",
            ));
        }
        let flags = u8::from_str_radix(parts[3], 16).map_err(|_| {
            AgentMeshError::new(ErrorCode::InvalidRequest, "The trace flags are invalid.")
        })?;
        Ok(Self {
            trace_id: parts[1].to_ascii_lowercase(),
            parent_id: parts[2].to_ascii_lowercase(),
            flags,
        })
    }

    /// Serializes the W3C `traceparent` value.
    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-{:02x}", self.trace_id, self.parent_id, self.flags)
    }

    /// Trace identifier safe for correlation.
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }
}

/// Validated low-cardinality dimensions.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Attributes(BTreeMap<String, String>);

impl Attributes {
    /// Builds dimensions from an explicit allowlist.
    ///
    /// # Errors
    ///
    /// Rejects excessive, unknown, sensitive, empty, or oversized attributes.
    pub fn new(
        values: BTreeMap<String, String>,
        allowed_keys: &BTreeSet<String>,
    ) -> Result<Self, AgentMeshError> {
        if values.len() > MAX_ATTRIBUTES {
            return Err(configuration("Telemetry has too many attributes."));
        }
        for (key, value) in &values {
            let normalized = key.to_ascii_lowercase();
            if !allowed_keys.contains(key)
                || sensitive_key(&normalized)
                || key.is_empty()
                || key.len() > 64
                || value.is_empty()
                || value.len() > 256
                || key.chars().any(char::is_control)
                || value.chars().any(char::is_control)
            {
                return Err(configuration(
                    "A telemetry attribute is unsafe or not allowlisted.",
                ));
            }
        }
        Ok(Self(values))
    }

    /// Read-only dimensions for exporters.
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

/// Structured event severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Diagnostic development detail.
    Debug,
    /// Normal operational event.
    Info,
    /// Recoverable abnormal condition.
    Warn,
    /// Failed operation requiring attention.
    Error,
}

/// Safe structured event. Arguments, outputs, credentials, and PII are not accepted fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryEvent {
    /// Severity.
    pub level: Level,
    /// Stable event name.
    pub name: String,
    /// Stable message/reason code, not free-form request data.
    pub message_code: String,
    /// Low-cardinality dimensions.
    pub attributes: Attributes,
    /// Optional trace correlation.
    pub trace_id: Option<String>,
    /// Event timestamp.
    pub at_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct MetricKey {
    name: String,
    attributes: Attributes,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Series {
    count: u64,
    sum: u64,
    max: u64,
}

/// Export-ready aggregate metric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricSnapshot {
    /// Stable metric name.
    pub name: String,
    /// Dimensions.
    pub attributes: Attributes,
    /// Observation count.
    pub count: u64,
    /// Saturating sum.
    pub sum: u64,
    /// Maximum observation.
    pub max: u64,
}

#[derive(Default)]
struct State {
    series: BTreeMap<MetricKey, Series>,
    events: VecDeque<TelemetryEvent>,
    dropped_series: u64,
    dropped_events: u64,
}

/// Bounded in-process recorder consumed by Prometheus/OpenTelemetry exporters.
pub struct TelemetryRecorder {
    max_series: usize,
    max_events: usize,
    state: Mutex<State>,
}

impl TelemetryRecorder {
    /// Creates a recorder with explicit hard bounds.
    ///
    /// # Errors
    ///
    /// Rejects zero or excessive capacities.
    pub fn new(max_series: usize, max_events: usize) -> Result<Self, AgentMeshError> {
        if max_series == 0
            || max_series > MAX_METRIC_SERIES
            || max_events == 0
            || max_events > MAX_TELEMETRY_EVENTS
        {
            return Err(configuration("Telemetry capacities are zero or unbounded."));
        }
        Ok(Self {
            max_series,
            max_events,
            state: Mutex::new(State::default()),
        })
    }

    /// Records a counter or duration observation with saturating arithmetic.
    ///
    /// # Errors
    ///
    /// Rejects invalid names; new series beyond the cap are dropped and reported as errors.
    pub fn record(
        &self,
        name: &str,
        attributes: Attributes,
        value: u64,
    ) -> Result<(), AgentMeshError> {
        validate_name(name)?;
        let key = MetricKey {
            name: name.into(),
            attributes,
        };
        let mut state = lock(&self.state);
        if !state.series.contains_key(&key) && state.series.len() >= self.max_series {
            state.dropped_series = state.dropped_series.saturating_add(1);
            return Err(AgentMeshError::new(
                ErrorCode::StorageUnavailable,
                "The telemetry series limit is reached.",
            ));
        }
        let series = state.series.entry(key).or_default();
        series.count = series.count.saturating_add(1);
        series.sum = series.sum.saturating_add(value);
        series.max = series.max.max(value);
        Ok(())
    }

    /// Emits a safe structured event into the bounded buffer.
    ///
    /// # Errors
    ///
    /// Rejects invalid stable event names and message codes.
    pub fn emit(&self, event: TelemetryEvent) -> Result<(), AgentMeshError> {
        validate_name(&event.name)?;
        validate_name(&event.message_code)?;
        let mut state = lock(&self.state);
        if state.events.len() == self.max_events {
            state.events.pop_front();
            state.dropped_events = state.dropped_events.saturating_add(1);
        }
        state.events.push_back(event);
        Ok(())
    }

    /// Returns metric series in deterministic order.
    pub fn metrics(&self) -> Vec<MetricSnapshot> {
        lock(&self.state)
            .series
            .iter()
            .map(|(key, series)| MetricSnapshot {
                name: key.name.clone(),
                attributes: key.attributes.clone(),
                count: series.count,
                sum: series.sum,
                max: series.max,
            })
            .collect()
    }

    /// Drains structured events.
    pub fn drain_events(&self) -> Vec<TelemetryEvent> {
        lock(&self.state).events.drain(..).collect()
    }

    /// Returns dropped series and event counters.
    pub fn dropped(&self) -> (u64, u64) {
        let state = lock(&self.state);
        (state.dropped_series, state.dropped_events)
    }
}

fn sensitive_key(key: &str) -> bool {
    [
        "token",
        "secret",
        "password",
        "authorization",
        "cookie",
        "argument",
        "output",
        "email",
    ]
    .iter()
    .any(|needle| key.contains(needle))
}
fn validate_name(value: &str) -> Result<(), AgentMeshError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-'))
        })
    {
        return Err(configuration("A telemetry name is invalid."));
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
