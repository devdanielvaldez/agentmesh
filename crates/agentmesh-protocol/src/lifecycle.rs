//! Modern discovery and legacy initialization payloads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CapabilitySet, Implementation, ProtocolVersion};

/// Metadata returned with an MCP result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultMeta {
    /// Optional self-reported server implementation information.
    #[serde(
        rename = "io.modelcontextprotocol/serverInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub server_info: Option<Implementation>,
    /// Extension metadata preserved verbatim.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Result returned by the modern `server/discover` method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResult {
    /// Completion discriminator defined by modern MCP discovery.
    pub result_type: DiscoverResultType,
    /// Protocol versions understood by the server.
    pub supported_versions: Vec<ProtocolVersion>,
    /// Capabilities offered by the server.
    pub capabilities: CapabilitySet,
    /// Optional guidance for clients and models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Standard result metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ResultMeta>,
    /// Future result fields preserved verbatim.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Completion state for server discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscoverResultType {
    /// Discovery completed synchronously.
    Complete,
}

/// Parameters sent by legacy clients to `initialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Latest protocol version offered by the client.
    pub protocol_version: ProtocolVersion,
    /// Legacy client capability advertisement.
    pub capabilities: CapabilitySet,
    /// Self-reported client software information.
    pub client_info: Implementation,
    /// Future fields preserved for compatibility.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Result returned by a legacy server from `initialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Protocol version selected by the server.
    pub protocol_version: ProtocolVersion,
    /// Legacy server capability advertisement.
    pub capabilities: CapabilitySet,
    /// Self-reported server software information.
    pub server_info: Implementation,
    /// Optional guidance for clients and models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Standard result metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ResultMeta>,
    /// Future fields preserved for compatibility.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}
