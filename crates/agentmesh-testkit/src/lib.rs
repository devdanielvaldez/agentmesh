//! Reusable deterministic MCP fixtures and failure injection.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{JsonRpcMessage, JsonRpcResponse, RequestId};
use agentmesh_transport::{McpTransport, TransportFuture, TransportKind};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Manually advanced monotonic clock for deterministic tests.
#[derive(Debug, Clone, Default)]
pub struct FakeClock(Arc<Mutex<Duration>>);
impl FakeClock {
    /// Current monotonic instant from the test epoch.
    pub fn now(&self) -> Duration {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Advances without sleeping.
    pub fn advance(&self, amount: Duration) {
        let mut now = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *now = now.saturating_add(amount);
    }
}

/// Small reproducible pseudo-random source for selection tests.
#[derive(Debug, Clone)]
pub struct SeededRandom(u64);
impl SeededRandom {
    /// Creates a generator. Zero is mapped to a non-zero seed.
    pub const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }
    /// Produces the next value using xorshift64*.
    pub fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        self.0 = value;
        value.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Selects a stable index.
    pub fn index(&mut self, length: usize) -> Option<usize> {
        let length = u64::try_from(length).ok()?;
        if length == 0 {
            return None;
        }
        usize::try_from(self.next_u64() % length).ok()
    }
}

/// Scripted result returned by a mock transport.
#[derive(Debug)]
pub enum ScriptedFrame {
    /// Return a protocol message.
    Message(JsonRpcMessage),
    /// Return a classified failure.
    Error(ErrorCode),
    /// End the stream cleanly.
    Disconnect,
}

/// In-memory MCP transport capturing writes and replaying scripted reads.
pub struct MockTransport {
    incoming: VecDeque<ScriptedFrame>,
    sent: Vec<JsonRpcMessage>,
    closed: bool,
}
impl MockTransport {
    /// Creates a bounded script.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when more than 10,000 frames are supplied.
    pub fn new(frames: impl IntoIterator<Item = ScriptedFrame>) -> Result<Self, AgentMeshError> {
        let incoming: VecDeque<_> = frames.into_iter().collect();
        if incoming.len() > 10_000 {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The mock transport script is too large.",
            ));
        }
        Ok(Self {
            incoming,
            sent: Vec::new(),
            closed: false,
        })
    }
    /// Messages captured from the subject under test.
    pub fn sent(&self) -> &[JsonRpcMessage] {
        &self.sent
    }
}
impl McpTransport for MockTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Stdio
    }
    fn send<'a>(&'a mut self, message: &'a JsonRpcMessage) -> TransportFuture<'a, ()> {
        Box::pin(async move {
            if self.closed {
                return Err(AgentMeshError::new(
                    ErrorCode::UpstreamUnavailable,
                    "The mock transport is closed.",
                ));
            }
            self.sent.push(message.clone());
            Ok(())
        })
    }
    fn receive(&mut self) -> TransportFuture<'_, Option<JsonRpcMessage>> {
        Box::pin(async move {
            match self.incoming.pop_front() {
                Some(ScriptedFrame::Message(message)) => Ok(Some(message)),
                Some(ScriptedFrame::Error(code)) => Err(AgentMeshError::new(
                    code,
                    "A scripted transport failure occurred.",
                )),
                Some(ScriptedFrame::Disconnect) | None => {
                    self.closed = true;
                    Ok(None)
                }
            }
        })
    }
    fn close(&mut self) -> TransportFuture<'_, ()> {
        Box::pin(async move {
            self.closed = true;
            Ok(())
        })
    }
}

/// Builds a minimal successful JSON-RPC fixture.
pub fn success(id: i64) -> JsonRpcMessage {
    JsonRpcMessage::Response(JsonRpcResponse::success(
        RequestId::Integer(id),
        serde_json::json!({}),
    ))
}

/// Assertion helper for stable public error codes.
///
/// # Panics
///
/// Panics when the result is successful or carries a different error code.
pub fn assert_error_code<T>(result: &Result<T, AgentMeshError>, expected: ErrorCode) {
    assert_eq!(
        result.as_ref().err().map(AgentMeshError::code),
        Some(expected),
        "unexpected AgentMesh error result"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_and_random_are_deterministic() {
        let clock = FakeClock::default();
        clock.advance(Duration::from_secs(2));
        assert_eq!(clock.now(), Duration::from_secs(2));
        assert_eq!(
            SeededRandom::new(7).next_u64(),
            SeededRandom::new(7).next_u64()
        );
    }
    #[tokio::test]
    async fn mock_scripts_disconnects() {
        let mut transport = MockTransport::new([ScriptedFrame::Disconnect]).unwrap();
        assert!(transport.receive().await.unwrap().is_none());
    }
}
