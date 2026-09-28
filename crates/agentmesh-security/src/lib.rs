//! Reusable SSRF, egress, integrity, payload, and redaction defenses.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

/// Default maximum validated JSON nesting depth.
pub const DEFAULT_MAX_JSON_DEPTH: usize = 64;
/// Maximum allowlist entries.
pub const MAX_EGRESS_RULES: usize = 10_000;

/// Validated outbound network policy.
#[derive(Debug, Clone)]
pub struct EgressPolicy {
    allowed_hosts: BTreeSet<String>,
    allowed_ports: BTreeSet<u16>,
    allow_http: bool,
}

impl EgressPolicy {
    /// Creates a bounded exact-host allowlist. A leading `.` rule matches subdomains only.
    ///
    /// # Errors
    ///
    /// Rejects empty/unbounded rules, invalid hosts, and zero ports.
    pub fn new(
        allowed_hosts: BTreeSet<String>,
        allowed_ports: BTreeSet<u16>,
        allow_http: bool,
    ) -> Result<Self, AgentMeshError> {
        if allowed_hosts.is_empty()
            || allowed_hosts.len() > MAX_EGRESS_RULES
            || allowed_ports.is_empty()
            || allowed_ports.len() > MAX_EGRESS_RULES
            || allowed_ports.contains(&0)
        {
            return Err(configuration("The egress allowlist is empty or unbounded."));
        }
        if allowed_hosts.iter().any(|host| {
            host.trim().is_empty()
                || host.len() > 253
                || host.chars().any(char::is_control)
                || host.contains('/')
                || host.contains('@')
        }) {
            return Err(configuration("An egress host rule is invalid."));
        }
        Ok(Self {
            allowed_hosts: allowed_hosts
                .into_iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
            allowed_ports,
            allow_http,
        })
    }

    /// Validates URL syntax, scheme, authority, allowlists, and every resolved address.
    ///
    /// # Errors
    ///
    /// Rejects credentials, fragments, disallowed hosts/ports, local ranges, or missing DNS results.
    pub fn validate_destination(
        &self,
        raw_url: &str,
        resolved: &[IpAddr],
    ) -> Result<PinnedDestination, AgentMeshError> {
        let url = Url::parse(raw_url).map_err(|_| blocked("The upstream URL is invalid."))?;
        if !matches!(url.scheme(), "https" | "http")
            || (url.scheme() == "http" && !self.allow_http)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(blocked(
                "The upstream URL contains a disallowed scheme or component.",
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| blocked("The upstream URL has no host."))?
            .to_ascii_lowercase();
        if host.parse::<IpAddr>().is_ok() || !self.host_allowed(&host) {
            return Err(blocked("The upstream host is not allowed."));
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| blocked("The upstream port is unknown."))?;
        if !self.allowed_ports.contains(&port)
            || resolved.is_empty()
            || resolved.iter().any(|address| !public_ip(*address))
        {
            return Err(blocked("The upstream address or port is not allowed."));
        }
        Ok(PinnedDestination {
            url,
            host,
            addresses: resolved.iter().copied().collect(),
        })
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.allowed_hosts.iter().any(|rule| {
            rule == host
                || rule
                    .strip_prefix('.')
                    .is_some_and(|suffix| host.ends_with(rule) && host.len() > suffix.len())
        })
    }
}

/// DNS-pinned destination used to detect rebinding during one request lifecycle.
#[derive(Debug, Clone)]
pub struct PinnedDestination {
    url: Url,
    host: String,
    addresses: BTreeSet<IpAddr>,
}

impl PinnedDestination {
    /// Parsed safe URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Safe hostname.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Validates that a subsequent DNS resolution remains public and overlaps the pinned set.
    ///
    /// # Errors
    ///
    /// Rejects empty, local, or completely changed address sets.
    pub fn validate_re_resolution(&self, addresses: &[IpAddr]) -> Result<(), AgentMeshError> {
        if addresses.is_empty()
            || addresses.iter().any(|address| !public_ip(*address))
            || !addresses
                .iter()
                .any(|address| self.addresses.contains(address))
        {
            return Err(blocked("The upstream DNS result changed unexpectedly."));
        }
        Ok(())
    }
}

/// Bounded JSON payload validator.
#[derive(Debug, Clone, Copy)]
pub struct PayloadGuard {
    /// Maximum encoded bytes.
    pub max_bytes: usize,
    /// Maximum object/array nesting.
    pub max_depth: usize,
}

impl PayloadGuard {
    /// Parses JSON after enforcing byte and nesting bounds.
    ///
    /// # Errors
    ///
    /// Returns payload or message errors without exposing input content.
    pub fn decode(&self, bytes: &[u8]) -> Result<Value, AgentMeshError> {
        if self.max_bytes == 0 || self.max_depth == 0 {
            return Err(configuration("Payload guard limits must be non-zero."));
        }
        if bytes.len() > self.max_bytes {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The payload exceeds its size limit.",
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|_| {
            AgentMeshError::new(ErrorCode::InvalidMessage, "The payload is not valid JSON.")
        })?;
        if json_depth(&value, 1) > self.max_depth {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The payload exceeds its nesting limit.",
            ));
        }
        Ok(value)
    }
}

/// Integrity check result used by discovery and quarantine policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityDecision {
    /// Definition matches its approved pin.
    Trusted,
    /// No pin exists and operator policy must decide.
    Unpinned,
    /// Existing pin changed and the service should be quarantined.
    Quarantine,
}

/// Calculates the canonical JSON SHA-256 fingerprint.
///
/// # Errors
///
/// Returns a schema error when serialization fails.
pub fn integrity_digest(value: &Value) -> Result<String, AgentMeshError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        AgentMeshError::new(
            ErrorCode::SchemaInvalid,
            "The definition cannot be canonicalized.",
        )
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// Compares an optional expected fingerprint without early exit.
///
/// # Errors
///
/// Returns a schema error for malformed expected pins.
pub fn verify_integrity(
    value: &Value,
    expected: Option<&str>,
) -> Result<IntegrityDecision, AgentMeshError> {
    let Some(expected) = expected else {
        return Ok(IntegrityDecision::Unpinned);
    };
    if expected.len() != 71
        || !expected.starts_with("sha256:")
        || !expected[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(configuration("The integrity pin is malformed."));
    }
    let actual = integrity_digest(value)?;
    Ok(
        if constant_time_eq(actual.as_bytes(), expected.as_bytes()) {
            IntegrityDecision::Trusted
        } else {
            IntegrityDecision::Quarantine
        },
    )
}

/// Removes sensitive and hop-by-hop metadata from a string header map.
pub fn sanitize_headers(headers: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    const ALLOWED: [&str; 7] = [
        "accept",
        "content-type",
        "mcp-protocol-version",
        "mcp-method",
        "mcp-name",
        "traceparent",
        "tracestate",
    ];
    headers
        .iter()
        .filter(|(name, _)| ALLOWED.contains(&name.to_ascii_lowercase().as_str()))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect()
}

/// Redacts values whose keys indicate credentials or personal secrets.
pub fn redact_metadata(metadata: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    metadata
        .iter()
        .map(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            let sensitive = [
                "authorization",
                "token",
                "secret",
                "password",
                "cookie",
                "api_key",
                "apikey",
            ]
            .iter()
            .any(|needle| normalized.contains(needle));
            (
                key.clone(),
                if sensitive {
                    "[REDACTED]".into()
                } else {
                    value.clone()
                },
            )
        })
        .collect()
}

fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => public_v4(value),
        IpAddr::V6(value) => public_v6(value),
    }
}

fn public_v4(value: Ipv4Addr) -> bool {
    let [a, b, _, _] = value.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b)))
}

fn public_v6(value: Ipv6Addr) -> bool {
    if let Some(mapped) = value.to_ipv4_mapped() {
        return public_v4(mapped);
    }
    let first = value.segments()[0];
    !(value.is_unspecified()
        || value.is_loopback()
        || value.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80)
}

fn json_depth(value: &Value, depth: usize) -> usize {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| json_depth(value, depth + 1))
            .max()
            .unwrap_or(depth),
        Value::Object(values) => values
            .values()
            .map(|value| json_depth(value, depth + 1))
            .max()
            .unwrap_or(depth),
        _ => depth,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
fn blocked(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::PermissionDenied, message)
}
fn configuration(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::ConfigurationInvalid, message)
}
