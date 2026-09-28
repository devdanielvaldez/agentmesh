//! Extensible client/server capability and implementation metadata.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Capabilities advertised by an MCP participant.
///
/// MCP capability keys evolve between revisions, so the protocol core preserves
/// unknown entries instead of discarding or prematurely interpreting them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet(BTreeMap<String, Value>);

impl CapabilitySet {
    /// Creates an empty capability set.
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Returns whether no capabilities were advertised.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Looks up a capability without interpreting its extension-specific value.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// Inserts or replaces a capability value.
    pub fn insert(&mut self, name: impl Into<String>, value: Value) -> Option<Value> {
        self.0.insert(name.into(), value)
    }

    /// Iterates over capabilities in stable lexical order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }
}

/// Self-reported client or server software information.
///
/// This data is informational and must not be used as a trusted security identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    /// Program or product name.
    pub name: String,
    /// Program version.
    pub version: String,
    /// Optional display-friendly title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Additional specification or vendor fields.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl Implementation {
    /// Creates minimal implementation metadata.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            title: None,
            extensions: BTreeMap::new(),
        }
    }
}
