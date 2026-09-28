//! Modern per-request MCP metadata.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CapabilitySet, Implementation, ProtocolVersion};

/// Reserved key containing the MCP version used by a request.
pub const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
/// Reserved key containing client capabilities relevant to a request.
pub const CLIENT_CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
/// Reserved key containing self-reported client software information.
pub const CLIENT_INFO_KEY: &str = "io.modelcontextprotocol/clientInfo";
/// Reserved key containing self-reported server software information.
pub const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// Metadata required on modern stateless MCP requests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestMeta {
    /// Protocol version used to interpret this request.
    #[serde(rename = "io.modelcontextprotocol/protocolVersion")]
    pub protocol_version: ProtocolVersion,
    /// Client capabilities relevant to this request.
    #[serde(rename = "io.modelcontextprotocol/clientCapabilities")]
    pub client_capabilities: CapabilitySet,
    /// Optional self-reported client software information.
    #[serde(
        rename = "io.modelcontextprotocol/clientInfo",
        skip_serializing_if = "Option::is_none"
    )]
    pub client_info: Option<Implementation>,
    /// Optional opaque progress token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_token: Option<Value>,
    /// Trace context or extension metadata preserved verbatim.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl RequestMeta {
    /// Creates the required metadata for a modern MCP request.
    pub fn new(protocol_version: ProtocolVersion, client_capabilities: CapabilitySet) -> Self {
        Self {
            protocol_version,
            client_capabilities,
            client_info: None,
            progress_token: None,
            extensions: BTreeMap::new(),
        }
    }
}
