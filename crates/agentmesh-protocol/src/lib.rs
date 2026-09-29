//! Transport-independent Model Context Protocol primitives for `AgentMesh`.
//!
//! The module supports the modern stateless MCP era introduced by protocol
//! version `2026-07-28` and explicitly models the legacy handshake era for
//! compatibility adapters. Network framing belongs in `agentmesh-transport`.

mod capability;
mod jsonrpc;
mod lifecycle;
mod messages;
mod metadata;
mod method;
mod name;
mod validation;
mod version;

pub use capability::{CapabilitySet, Implementation};
pub use jsonrpc::{
    JSONRPC_VERSION, JsonRpcErrorObject, JsonRpcFailure, JsonRpcMessage, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, JsonRpcSuccess, RequestId,
};
pub use lifecycle::{
    DiscoverResult, DiscoverResultType, InitializeParams, InitializeResult, ResultMeta,
};
pub use messages::{
    CallToolParams, CallToolResult, ContentBlock, GetPromptParams, ListResult, PromptDefinition,
    ReadResourceParams, ResourceContents, ResourceDefinition, TaskReference, TaskStatus,
    ToolDefinition,
};
pub use metadata::{
    CLIENT_CAPABILITIES_KEY, CLIENT_INFO_KEY, PROTOCOL_VERSION_KEY, RequestMeta, SERVER_INFO_KEY,
};
pub use method::McpMethod;
pub use name::McpName;
pub use validation::{ProtocolLimits, decode_message};
pub use version::{
    LATEST_PROTOCOL_VERSION, ProtocolEra, ProtocolVersion, SupportedVersions, negotiate_version,
};
