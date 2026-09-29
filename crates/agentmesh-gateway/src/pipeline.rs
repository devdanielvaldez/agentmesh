//! Ordered, fail-closed data-plane orchestration contracts.

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::JsonRpcMessage;
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

/// Required gateway processing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PipelineStage {
    /// Validate transport and protocol bounds.
    Admission,
    /// Establish caller identity.
    Authentication,
    /// Enforce RBAC and contextual policy.
    Authorization,
    /// Enforce rate, concurrency, and quota limits.
    Limits,
    /// Resolve the requested capability.
    Resolution,
    /// Select a logical route and endpoint pool.
    Routing,
    /// Apply deadline, breaker, and retry guards.
    Resilience,
    /// Forward to the selected upstream.
    Proxy,
    /// Validate the upstream response.
    ResponseValidation,
    /// Emit accounting, telemetry, and audit records.
    Accounting,
}

impl PipelineStage {
    const ORDER: [Self; 10] = [
        Self::Admission,
        Self::Authentication,
        Self::Authorization,
        Self::Limits,
        Self::Resolution,
        Self::Routing,
        Self::Resilience,
        Self::Proxy,
        Self::ResponseValidation,
        Self::Accounting,
    ];
}

/// Mutable request-local state; never shared between tenants.
#[derive(Debug, Clone)]
pub struct PipelineContext {
    /// Explicit tenant identifier.
    pub tenant: String,
    /// Trace correlation identifier.
    pub trace_id: String,
    /// Validated MCP message.
    pub message: JsonRpcMessage,
    /// Bounded middleware annotations.
    pub annotations: BTreeMap<String, String>,
}

/// Future returned by object-safe middleware.
pub type PipelineFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AgentMeshError>> + Send + 'a>>;

/// One independently testable request-pipeline stage.
pub trait RequestMiddleware: Send + Sync {
    /// Executes the stage or fails closed.
    fn handle<'a>(&'a self, context: &'a mut PipelineContext) -> PipelineFuture<'a>;
}

/// Immutable ordered middleware snapshot.
#[derive(Default)]
pub struct GatewayPipeline {
    stages: BTreeMap<PipelineStage, Arc<dyn RequestMiddleware>>,
}
impl GatewayPipeline {
    /// Builds a complete pipeline; missing stages are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] unless every canonical stage is present.
    pub fn new(
        stages: impl IntoIterator<Item = (PipelineStage, Arc<dyn RequestMiddleware>)>,
    ) -> Result<Self, AgentMeshError> {
        let stages: BTreeMap<_, _> = stages.into_iter().collect();
        if PipelineStage::ORDER
            .iter()
            .any(|stage| !stages.contains_key(stage))
        {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "Every gateway pipeline stage must be configured.",
            ));
        }
        Ok(Self { stages })
    }
    /// Runs stages in the security-sensitive canonical order.
    ///
    /// # Errors
    ///
    /// Returns the first stage failure or a context resource-limit error.
    pub async fn execute(&self, context: &mut PipelineContext) -> Result<(), AgentMeshError> {
        if context.tenant.is_empty() || context.trace_id.is_empty() {
            return Err(AgentMeshError::new(
                ErrorCode::InvalidRequest,
                "Gateway context requires tenant and trace identifiers.",
            ));
        }
        for stage in PipelineStage::ORDER {
            self.stages[&stage].handle(context).await?;
            if context.annotations.len() > 128
                || context
                    .annotations
                    .iter()
                    .any(|(key, value)| key.len() > 128 || value.len() > 1024)
            {
                return Err(AgentMeshError::new(
                    ErrorCode::PayloadTooLarge,
                    "Gateway middleware annotations exceed their bounds.",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_protocol::{JsonRpcRequest, McpMethod, RequestId};
    use std::sync::Mutex;
    struct Record(PipelineStage, Arc<Mutex<Vec<PipelineStage>>>);
    impl RequestMiddleware for Record {
        fn handle<'a>(&'a self, _: &'a mut PipelineContext) -> PipelineFuture<'a> {
            Box::pin(async move {
                self.1.lock().unwrap().push(self.0);
                Ok(())
            })
        }
    }
    #[tokio::test]
    async fn executes_the_canonical_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let pipeline = GatewayPipeline::new(PipelineStage::ORDER.map(|stage| {
            (
                stage,
                Arc::new(Record(stage, Arc::clone(&calls))) as Arc<dyn RequestMiddleware>,
            )
        }))
        .unwrap();
        let mut context = PipelineContext {
            tenant: "acme".into(),
            trace_id: "trace".into(),
            message: JsonRpcMessage::Request(JsonRpcRequest::new(
                RequestId::Integer(1),
                McpMethod::Ping,
                None,
            )),
            annotations: BTreeMap::new(),
        };
        pipeline.execute(&mut context).await.unwrap();
        assert_eq!(*calls.lock().unwrap(), PipelineStage::ORDER);
    }
}
