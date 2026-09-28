//! Typed `AgentMesh` configuration with validation at startup.

use std::{fs, net::IpAddr, path::Path};

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
}
