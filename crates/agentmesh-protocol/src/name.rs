//! Validated MCP capability names.

use std::{fmt, str::FromStr};

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Maximum name size accepted by `AgentMesh`.
pub const MAX_NAME_BYTES: usize = 128;

/// A validated MCP tool, resource, or prompt name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct McpName(String);

impl McpName {
    /// Validates and creates an MCP name.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when a non-empty name is too large, does not
    /// begin and end with an ASCII alphanumeric character, or contains a
    /// character other than an ASCII alphanumeric character, `.`, `-`, or `_`.
    pub fn parse(value: impl Into<String>) -> Result<Self, AgentMeshError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let valid_boundary = bytes.is_empty()
            || (bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                && bytes.last().is_some_and(u8::is_ascii_alphanumeric));
        let valid_characters = bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));

        if bytes.len() > MAX_NAME_BYTES || !valid_boundary || !valid_characters {
            return Err(AgentMeshError::new(
                ErrorCode::SchemaInvalid,
                "The MCP name is invalid.",
            ));
        }

        Ok(Self(value))
    }

    /// Returns the validated wire value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for McpName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for McpName {
    type Err = AgentMeshError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for McpName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for McpName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}
