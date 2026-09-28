//! MCP protocol versions, eras, and deterministic negotiation.

use std::{collections::BTreeSet, fmt, str::FromStr};

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Latest protocol revision implemented by this module.
pub const LATEST_PROTOCOL_VERSION: &str = "2026-07-28";
const FIRST_MODERN_VERSION: &str = "2026-07-28";

/// MCP wire behavior family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolEra {
    /// Connection-scoped initialization used through `2025-11-25`.
    Legacy,
    /// Stateless per-request metadata used from `2026-07-28` onward.
    Modern,
}

/// Validated date-based MCP protocol revision.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProtocolVersion(String);

impl ProtocolVersion {
    /// Parses the `YYYY-MM-DD` version form used by MCP revisions.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the value is not a valid calendar date
    /// in the canonical ten-byte representation.
    pub fn parse(value: impl Into<String>) -> Result<Self, AgentMeshError> {
        let value = value.into();
        if !is_valid_date_version(&value) {
            return Err(AgentMeshError::new(
                ErrorCode::UnsupportedProtocolVersion,
                "The MCP protocol version is invalid or unsupported.",
            ));
        }
        Ok(Self(value))
    }

    /// Returns the latest protocol version supported by this crate.
    pub fn latest() -> Self {
        Self(LATEST_PROTOCOL_VERSION.into())
    }

    /// Returns the canonical wire value.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the wire behavior family for this revision.
    pub fn era(&self) -> ProtocolEra {
        if self.0.as_str() >= FIRST_MODERN_VERSION {
            ProtocolEra::Modern
        } else {
            ProtocolEra::Legacy
        }
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ProtocolVersion {
    type Err = AgentMeshError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for ProtocolVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Ordered, duplicate-free versions supported by one participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedVersions(BTreeSet<ProtocolVersion>);

impl SupportedVersions {
    /// Validates a non-empty collection of protocol revisions.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] if no supported version is supplied.
    pub fn new(
        versions: impl IntoIterator<Item = ProtocolVersion>,
    ) -> Result<Self, AgentMeshError> {
        let versions = versions.into_iter().collect::<BTreeSet<_>>();
        if versions.is_empty() {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "At least one supported MCP protocol version is required.",
            ));
        }
        Ok(Self(versions))
    }

    /// Returns a set containing only the latest version implemented here.
    pub fn latest_only() -> Self {
        let mut versions = BTreeSet::new();
        versions.insert(ProtocolVersion::latest());
        Self(versions)
    }

    /// Iterates from oldest to newest.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &ProtocolVersion> {
        self.0.iter()
    }
}

/// Selects the newest version supported by both participants.
///
/// # Errors
///
/// Returns [`AgentMeshError`] with [`ErrorCode::UnsupportedProtocolVersion`]
/// when no version is shared.
pub fn negotiate_version(
    client: &SupportedVersions,
    server: &SupportedVersions,
) -> Result<ProtocolVersion, AgentMeshError> {
    client
        .iter()
        .rev()
        .find(|version| server.0.contains(*version))
        .cloned()
        .ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::UnsupportedProtocolVersion,
                "Client and server do not share a supported MCP protocol version.",
            )
        })
}

fn is_valid_date_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return false;
    }

    let year = value[0..4].parse::<u16>().ok();
    let month = value[5..7].parse::<u8>().ok();
    let day = value[8..10].parse::<u8>().ok();
    let Some((year, month, day)) = year.zip(month).zip(day).map(|((y, m), d)| (y, m, d)) else {
        return false;
    };

    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let maximum_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=maximum_day).contains(&day)
}
