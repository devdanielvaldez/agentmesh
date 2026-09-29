//! Bounded transport bindings for the Model Context Protocol.
//!
//! This crate owns message framing and transport-specific negotiation. MCP
//! message meaning, versions, and validation remain in `agentmesh-protocol`.

mod http_binding;
mod session;
mod sse;
mod stdio;
mod supervisor;
mod transport;
mod websocket;

pub use http_binding::{
    MCP_PROTOCOL_VERSION_HEADER, ResponseMode, resolve_http_protocol_version, select_response_mode,
    validate_json_content_type,
};
pub use session::{Cancellation, SessionId, SessionRegistry};
pub use sse::{decode_sse_event, encode_sse_event};
pub use stdio::StdioTransport;
pub use supervisor::{ChildProcessTransport, ProcessSpec};
pub use transport::{McpTransport, TransportFuture, TransportKind};
pub use websocket::{decode_websocket_frame, encode_websocket_frame};
