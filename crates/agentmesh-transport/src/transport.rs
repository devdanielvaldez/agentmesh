//! Common asynchronous transport contract.

use std::{future::Future, pin::Pin};

use agentmesh_error::AgentMeshError;
use agentmesh_protocol::JsonRpcMessage;

/// Heap-backed future returned by object-safe transport methods.
pub type TransportFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentMeshError>> + Send + 'a>>;

/// MCP transport binding in use for a connection or request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransportKind {
    /// Newline-delimited JSON-RPC over process standard I/O.
    Stdio,
    /// MCP Streamable HTTP binding.
    StreamableHttp,
    /// Server-Sent Events response stream.
    ServerSentEvents,
    /// One MCP message per WebSocket text frame.
    WebSocket,
}

/// Object-safe asynchronous message transport.
pub trait McpTransport: Send {
    /// Identifies the binding implemented by this instance.
    fn kind(&self) -> TransportKind;

    /// Sends one complete MCP message.
    fn send<'a>(&'a mut self, message: &'a JsonRpcMessage) -> TransportFuture<'a, ()>;

    /// Receives one message, or `None` after a clean end of stream.
    fn receive(&mut self) -> TransportFuture<'_, Option<JsonRpcMessage>>;

    /// Gracefully closes the writable side of the transport.
    fn close(&mut self) -> TransportFuture<'_, ()>;
}
