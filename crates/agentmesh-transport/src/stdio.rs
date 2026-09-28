//! Newline-delimited MCP framing over asynchronous standard I/O streams.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, ProtocolLimits, decode_message};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::{McpTransport, TransportFuture, TransportKind};

/// Bounded newline-delimited MCP transport.
pub struct StdioTransport<R, W> {
    reader: BufReader<R>,
    writer: W,
    limits: ProtocolLimits,
    closed: bool,
}

impl<R, W> StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    /// Creates a stdio binding over any asynchronous reader/writer pair.
    pub fn new(reader: R, writer: W, limits: ProtocolLimits) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            limits,
            closed: false,
        }
    }

    /// Sends one compact JSON message followed by a newline.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the transport is closed, serialization
    /// fails, the frame exceeds its bound, or the writer fails.
    pub async fn send_message(&mut self, message: &JsonRpcMessage) -> Result<(), AgentMeshError> {
        if self.closed {
            return Err(closed_error());
        }
        let encoded = serde_json::to_vec(message).map_err(|error| {
            AgentMeshError::with_source(
                ErrorCode::Internal,
                "The MCP message could not be encoded.",
                error,
            )
        })?;
        if encoded.len() > self.limits.max_message_bytes {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The MCP message exceeds the configured size limit.",
            ));
        }

        self.writer.write_all(&encoded).await.map_err(io_error)?;
        self.writer.write_all(b"\n").await.map_err(io_error)?;
        self.writer.flush().await.map_err(io_error)
    }

    /// Receives and validates one newline-delimited JSON message.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] for I/O failures, empty frames, oversized
    /// frames, malformed JSON, or invalid MCP envelopes.
    pub async fn receive_message(&mut self) -> Result<Option<JsonRpcMessage>, AgentMeshError> {
        if self.closed {
            return Ok(None);
        }

        let frame_bound = self.limits.max_message_bytes.saturating_add(1);
        let mut frame = Vec::with_capacity(frame_bound.min(8 * 1024));
        let mut limited_reader = (&mut self.reader).take(frame_bound as u64);
        let bytes_read = limited_reader
            .read_until(b'\n', &mut frame)
            .await
            .map_err(io_error)?;

        if bytes_read == 0 {
            self.closed = true;
            return Ok(None);
        }
        if frame.len() > self.limits.max_message_bytes {
            self.closed = true;
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The stdio MCP frame exceeds the configured size limit.",
            ));
        }

        if frame.last() == Some(&b'\n') {
            frame.pop();
        }
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
        if frame.is_empty() {
            return Err(AgentMeshError::new(
                ErrorCode::InvalidMessage,
                "An empty stdio MCP frame is not valid.",
            ));
        }

        decode_message(&frame, self.limits).map(Some)
    }

    /// Shuts down the writable stream. Repeated calls are safe.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the underlying writer cannot shut down.
    pub async fn shutdown(&mut self) -> Result<(), AgentMeshError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.writer.shutdown().await.map_err(io_error)
    }

    /// Returns whether this transport has reached EOF, exceeded a fatal frame
    /// bound, or been explicitly closed.
    pub const fn is_closed(&self) -> bool {
        self.closed
    }
}

impl<R, W> McpTransport for StdioTransport<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    fn kind(&self) -> TransportKind {
        TransportKind::Stdio
    }

    fn send<'a>(&'a mut self, message: &'a JsonRpcMessage) -> TransportFuture<'a, ()> {
        Box::pin(async move { self.send_message(message).await })
    }

    fn receive(&mut self) -> TransportFuture<'_, Option<JsonRpcMessage>> {
        Box::pin(async move { self.receive_message().await })
    }

    fn close(&mut self) -> TransportFuture<'_, ()> {
        Box::pin(async move { self.shutdown().await })
    }
}

fn io_error(source: std::io::Error) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::UpstreamConnectionFailed,
        "The MCP transport encountered an I/O failure.",
        source,
    )
}

fn closed_error() -> AgentMeshError {
    AgentMeshError::new(
        ErrorCode::UpstreamUnavailable,
        "The MCP transport is closed.",
    )
}
