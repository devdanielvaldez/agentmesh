//! Transport-independent Model Context Protocol primitives for `AgentMesh`.
//!
//! The module supports the modern stateless MCP era introduced by protocol
//! version `2026-07-28` and explicitly models the legacy handshake era for
//! compatibility adapters. Network framing belongs in `agentmesh-transport`.

mod capability;
mod capability_graph;
mod capability_package;
mod certification;
mod execution_receipt;
mod jsonrpc;
mod lifecycle;
mod messages;
mod metadata;
mod method;
mod name;
mod recovery;
mod validation;
mod version;

pub use capability::{CapabilitySet, Implementation};
pub use capability_graph::{
    CapabilityMatch, CapabilityNegotiationError, CapabilityNegotiationRequest, CapabilityOffer,
    CapabilityPlanError, ImplementationSelectionError, discover_capabilities,
    negotiate_capability_offer, resolve_capability_plan, select_implementation,
};
pub use capability_package::{
    ApprovalCheckpoint, AuthorityRequirements, CAPABILITY_PACKAGE_VERSION, CapabilityContract,
    CapabilityDefinition, CapabilityPackage, CapabilityRequirement, CapabilityValidationError,
    EffectKind, EvidenceClaim, EvidenceType, Idempotency, ImplementationBinding,
    ImplementationKind, Provenance, ProvenanceSource, RecoveryStrategy,
};
pub use certification::{
    CertificationCheck, CertificationDecision, CertificationLevel, CertificationReport,
    evaluate_certification,
};
pub use execution_receipt::{
    EvidenceVerification, ExecutionEvidence, ExecutionReceipt, ExecutionStatus,
    verify_execution_receipt,
};
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
pub use recovery::{RecoveryAction, RecoveryContext, RecoveryDecision, decide_recovery};
pub use validation::{ProtocolLimits, decode_message};
pub use version::{
    LATEST_PROTOCOL_VERSION, ProtocolEra, ProtocolVersion, SupportedVersions, negotiate_version,
};
