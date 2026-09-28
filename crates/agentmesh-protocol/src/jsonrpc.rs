//! JSON-RPC 2.0 envelopes used by MCP.

use std::fmt;

use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::Value;

use crate::{McpMethod, RequestMeta};

/// JSON-RPC revision required by MCP.
pub const JSONRPC_VERSION: &str = "2.0";

/// A JSON-RPC identifier accepted by MCP.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// String identifier.
    String(String),
    /// Integer identifier.
    Integer(i64),
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => formatter.write_str(value),
            Self::Integer(value) => value.fmt(formatter),
        }
    }
}

/// An MCP request expecting a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest {
    #[serde(rename = "jsonrpc", deserialize_with = "deserialize_jsonrpc_version")]
    jsonrpc: String,
    /// Correlation identifier copied into the response.
    pub id: RequestId,
    /// MCP method being invoked.
    pub method: McpMethod,
    /// Optional method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Creates a valid JSON-RPC 2.0 MCP request.
    pub fn new(id: RequestId, method: McpMethod, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            method,
            params,
        }
    }

    /// Parses the required modern `_meta` object from request parameters.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when parameters, `_meta`, or any required
    /// modern protocol metadata field is absent or invalid.
    pub fn request_meta(&self) -> Result<RequestMeta, AgentMeshError> {
        let metadata = self
            .params
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|params| params.get("_meta"))
            .cloned()
            .ok_or_else(|| invalid_request("Modern MCP requests require a _meta object."))?;
        serde_json::from_value(metadata)
            .map_err(|_| invalid_request("Modern MCP request metadata is invalid."))
    }
}

/// An MCP notification that does not receive a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcNotification {
    #[serde(rename = "jsonrpc", deserialize_with = "deserialize_jsonrpc_version")]
    jsonrpc: String,
    /// MCP notification method.
    pub method: McpMethod,
    /// Optional notification parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Creates a valid JSON-RPC 2.0 MCP notification.
    pub fn new(method: McpMethod, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method,
            params,
        }
    }

    /// Parses modern `_meta` from notification parameters when present.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when parameters, `_meta`, or required
    /// protocol metadata is absent or invalid.
    pub fn request_meta(&self) -> Result<RequestMeta, AgentMeshError> {
        let metadata = self
            .params
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|params| params.get("_meta"))
            .cloned()
            .ok_or_else(|| invalid_request("Modern MCP notifications require a _meta object."))?;
        serde_json::from_value(metadata)
            .map_err(|_| invalid_request("Modern MCP notification metadata is invalid."))
    }
}

/// Protocol error information returned inside a JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRpcErrorObject {
    /// Numeric JSON-RPC or MCP error code.
    pub code: i32,
    /// Concise human-readable error message.
    pub message: String,
    /// Optional structured error metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcErrorObject {
    /// Creates a protocol error object.
    pub fn new(code: i32, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }
}

/// A successful or failed JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    /// Successful method result.
    Success(JsonRpcSuccess),
    /// Failed method result.
    Error(JsonRpcFailure),
}

impl JsonRpcResponse {
    /// Creates a successful response.
    pub fn success(id: RequestId, result: Value) -> Self {
        Self::Success(JsonRpcSuccess {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result,
        })
    }

    /// Creates an error response.
    pub fn error(id: RequestId, error: JsonRpcErrorObject) -> Self {
        Self::Error(JsonRpcFailure {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            error,
        })
    }

    /// Returns the request identifier shared by either response variant.
    pub const fn id(&self) -> &RequestId {
        match self {
            Self::Success(response) => &response.id,
            Self::Error(response) => &response.id,
        }
    }

    /// Returns whether this response contains an error.
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

/// Successful JSON-RPC response fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcSuccess {
    #[serde(rename = "jsonrpc", deserialize_with = "deserialize_jsonrpc_version")]
    jsonrpc: String,
    /// Identifier of the corresponding request.
    pub id: RequestId,
    /// Method-specific result.
    pub result: Value,
}

/// Failed JSON-RPC response fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcFailure {
    #[serde(rename = "jsonrpc", deserialize_with = "deserialize_jsonrpc_version")]
    jsonrpc: String,
    /// Identifier of the corresponding request.
    pub id: RequestId,
    /// Structured failure.
    pub error: JsonRpcErrorObject,
}

/// Any JSON-RPC object carried by MCP.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// Request expecting a response.
    Request(JsonRpcRequest),
    /// One-way notification.
    Notification(JsonRpcNotification),
    /// Success or error response.
    Response(JsonRpcResponse),
}

impl TryFrom<Value> for JsonRpcMessage {
    type Error = AgentMeshError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let object = value
            .as_object()
            .ok_or_else(|| invalid_request("A JSON-RPC message must be an object."))?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
            return Err(invalid_request("The JSON-RPC version must be 2.0."));
        }

        let has_method = object.contains_key("method");
        let has_id = object.contains_key("id");
        let has_result = object.contains_key("result");
        let has_error = object.contains_key("error");

        match (has_method, has_id, has_result, has_error) {
            (true, true, false, false) => serde_json::from_value(value)
                .map(Self::Request)
                .map_err(|_| invalid_request("The JSON-RPC request is invalid.")),
            (true, false, false, false) => serde_json::from_value(value)
                .map(Self::Notification)
                .map_err(|_| invalid_request("The JSON-RPC notification is invalid.")),
            (false, true, true, false) | (false, true, false, true) => {
                serde_json::from_value(value)
                    .map(Self::Response)
                    .map_err(|_| invalid_request("The JSON-RPC response is invalid."))
            }
            _ => Err(invalid_request(
                "The JSON-RPC message contains conflicting or missing fields.",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for JsonRpcMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::try_from(value).map_err(de::Error::custom)
    }
}

fn invalid_request(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::InvalidRequest, message)
}

fn deserialize_jsonrpc_version<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let version = String::deserialize(deserializer)?;
    if version == JSONRPC_VERSION {
        Ok(version)
    } else {
        Err(de::Error::custom("JSON-RPC version must be 2.0"))
    }
}
