//! Typed `AgentMesh` configuration with validation at startup.

use std::{
    collections::BTreeMap,
    fs,
    net::IpAddr,
    path::Path,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Complete process configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Gateway listener settings.
    pub gateway: GatewayConfig,
    /// Logging and tracing settings.
    pub telemetry: TelemetryConfig,
}

impl Config {
    /// Reads, parses, and validates a YAML configuration file.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the file cannot be read, parsed, or validated.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let contents = fs::read_to_string(path).map_err(ConfigError::Read)?;
        let config: Self = serde_yaml::from_str(&contents).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Loads YAML then applies explicit `AGENTMESH_*` environment values.
    ///
    /// Accepting an iterator keeps precedence deterministic and tests race-free.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when a source cannot be parsed or validated.
    pub fn from_sources(
        path: impl AsRef<Path>,
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let mut config = Self::from_path(path)?;
        let values: BTreeMap<_, _> = environment.into_iter().collect();
        if let Some(host) = values.get("AGENTMESH_GATEWAY_HOST") {
            config.gateway.host = host
                .parse()
                .map_err(|_| ConfigError::Validation("AGENTMESH_GATEWAY_HOST is invalid".into()))?;
        }
        if let Some(port) = values.get("AGENTMESH_GATEWAY_PORT") {
            config.gateway.port = port
                .parse()
                .map_err(|_| ConfigError::Validation("AGENTMESH_GATEWAY_PORT is invalid".into()))?;
        }
        if let Some(filter) = values.get("AGENTMESH_LOG_FILTER") {
            config.telemetry.filter.clone_from(filter);
        }
        config.validate()?;
        Ok(config)
    }

    /// Returns a machine-readable schema used by editors and CI.
    pub fn json_schema() -> serde_json::Value {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "gateway": {"type":"object", "additionalProperties":false},
                "telemetry": {"type":"object", "additionalProperties":false}
            }
        })
    }

    /// Rejects invalid settings before the gateway begins accepting traffic.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Validation`] when a value cannot be used safely.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.gateway.port == 0 {
            return Err(ConfigError::Validation(
                "gateway.port must be greater than zero".into(),
            ));
        }
        if let Some(upstream) = &self.gateway.upstream {
            if upstream.url.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "gateway.upstream.url must not be empty".into(),
                ));
            }
            if upstream.request_timeout_ms == 0 {
                return Err(ConfigError::Validation(
                    "gateway.upstream.request_timeout_ms must be greater than zero".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Opaque provider/key reference; never a plaintext credential.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretReference {
    /// Provider name.
    pub provider: String,
    /// Provider-local key.
    pub key: String,
}

impl std::fmt::Debug for SecretReference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretReference")
            .field("provider", &self.provider)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// Immutable configuration view swapped after successful validation.
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    /// Monotonic local revision.
    pub revision: u64,
    /// Validated configuration.
    pub config: Arc<Config>,
}

/// Thread-safe hot-reload holder that preserves the last valid snapshot.
#[derive(Debug, Clone)]
pub struct ConfigManager(Arc<RwLock<ConfigSnapshot>>);

impl ConfigManager {
    /// Creates revision one from validated configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the initial configuration is invalid.
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self(Arc::new(RwLock::new(ConfigSnapshot {
            revision: 1,
            config: Arc::new(config),
        }))))
    }

    /// Returns a consistent immutable snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Unavailable`] if the snapshot lock is poisoned.
    pub fn snapshot(&self) -> Result<ConfigSnapshot, ConfigError> {
        self.0
            .read()
            .map_err(|_| ConfigError::Unavailable)
            .map(|value| value.clone())
    }

    /// Validates and swaps the entire configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when validation or the atomic swap fails.
    pub fn reload(&self, config: Config) -> Result<ConfigSnapshot, ConfigError> {
        config.validate()?;
        let mut current = self.0.write().map_err(|_| ConfigError::Unavailable)?;
        let next = ConfigSnapshot {
            revision: current.revision.checked_add(1).ok_or_else(|| {
                ConfigError::Validation("configuration revision exhausted".into())
            })?,
            config: Arc::new(config),
        };
        *current = next.clone();
        Ok(next)
    }
}

/// Gateway HTTP listener settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayConfig {
    /// Interface to bind.
    pub host: IpAddr,
    /// TCP port to bind.
    pub port: u16,
    /// Optional static upstream for the first functional proxy milestone.
    pub upstream: Option<UpstreamConfig>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::from([0, 0, 0, 0]),
            port: 8080,
            upstream: None,
        }
    }
}

/// Static upstream used before the server registry and router are introduced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpstreamConfig {
    /// Streamable HTTP MCP endpoint URL.
    pub url: String,
    /// Allows plain HTTP for explicitly configured local development endpoints.
    pub allow_insecure_http: bool,
    /// Per-request deadline in milliseconds.
    pub request_timeout_ms: u64,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            allow_insecure_http: false,
            request_timeout_ms: 30_000,
        }
    }
}

/// Telemetry output settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    /// Emit newline-delimited JSON logs.
    pub json: bool,
    /// Default tracing filter when `RUST_LOG` is absent.
    pub filter: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            json: false,
            filter: "agentmesh=info,tower_http=info".into(),
        }
    }
}

/// Configuration loading or validation failure.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The requested file could not be read.
    #[error("could not read configuration: {0}")]
    Read(#[source] std::io::Error),
    /// YAML did not match the configuration schema.
    #[error("invalid configuration syntax: {0}")]
    Parse(#[source] serde_yaml::Error),
    /// A parsed value is not operationally valid.
    #[error("invalid configuration: {0}")]
    Validation(String),
    /// The live snapshot lock is unavailable.
    #[error("configuration snapshot is unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_configuration_is_valid() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn rejects_zero_port() {
        let mut config = Config::default();
        config.gateway.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_empty_upstream_url() {
        let mut config = Config::default();
        config.gateway.upstream = Some(UpstreamConfig::default());
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_zero_upstream_timeout() {
        let mut config = Config::default();
        config.gateway.upstream = Some(UpstreamConfig {
            url: "https://example.com/mcp".into(),
            request_timeout_ms: 0,
            ..UpstreamConfig::default()
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn reload_is_atomic_and_revisioned() {
        let manager = ConfigManager::new(Config::default()).unwrap();
        let mut next = Config::default();
        next.gateway.port = 9090;
        assert_eq!(manager.reload(next).unwrap().revision, 2);
        let mut invalid = Config::default();
        invalid.gateway.port = 0;
        assert!(manager.reload(invalid).is_err());
        assert_eq!(manager.snapshot().unwrap().config.gateway.port, 9090);
    }
}
