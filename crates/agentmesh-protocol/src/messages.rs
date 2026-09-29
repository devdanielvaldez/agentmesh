//! Typed tool, resource, prompt, and long-running task payloads.

use crate::McpName;
use agentmesh_error::{AgentMeshError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum definitions allowed in one list response.
pub const MAX_LIST_ITEMS: usize = 10_000;

/// Normalized MCP tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDefinition {
    /// Validated tool name.
    pub name: McpName,
    /// Optional human description.
    pub description: Option<String>,
    /// JSON Schema for tool arguments.
    pub input_schema: Value,
    /// Optional output JSON Schema.
    pub output_schema: Option<Value>,
}

/// Parameters for `tools/call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallToolParams {
    /// Tool to invoke.
    pub name: McpName,
    /// Structured arguments.
    #[serde(default)]
    pub arguments: BTreeMap<String, Value>,
}

/// One model-visible content item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    /// UTF-8 text.
    Text {
        /// Text value.
        text: String,
    },
    /// Opaque structured output.
    Json {
        /// Structured JSON value.
        value: Value,
    },
    /// Resource reference.
    ResourceLink {
        /// Referenced resource URI.
        uri: String,
        /// Display name.
        name: String,
    },
}

/// Result of `tools/call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallToolResult {
    /// Returned content.
    pub content: Vec<ContentBlock>,
    /// Whether the tool reported a domain error.
    #[serde(default)]
    pub is_error: bool,
    /// Optional long-running task.
    pub task: Option<TaskReference>,
}

/// Resource advertised by an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceDefinition {
    /// Resource URI.
    pub uri: String,
    /// Display name.
    pub name: String,
    /// Optional media type.
    pub mime_type: Option<String>,
    /// Optional description.
    pub description: Option<String>,
}

/// Parameters for `resources/read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadResourceParams {
    /// Resource URI.
    pub uri: String,
}

/// Resource body returned by the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceContents {
    /// Resource URI.
    pub uri: String,
    /// Media type.
    pub mime_type: Option<String>,
    /// Text body.
    pub text: Option<String>,
    /// Base64 body.
    pub blob: Option<String>,
}

/// Prompt advertised by an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDefinition {
    /// Prompt name.
    pub name: McpName,
    /// Optional description.
    pub description: Option<String>,
    /// Declared argument names.
    #[serde(default)]
    pub arguments: Vec<String>,
}

/// Parameters for `prompts/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetPromptParams {
    /// Prompt name.
    pub name: McpName,
    /// Supplied prompt arguments.
    #[serde(default)]
    pub arguments: BTreeMap<String, String>,
}

/// Long-running task lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is executing.
    Working,
    /// Task is waiting for caller input.
    InputRequired,
    /// Task finished successfully.
    Completed,
    /// Task finished with an error.
    Failed,
    /// Task was cancelled.
    Cancelled,
}

/// Task reference returned by a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskReference {
    /// Server task identifier.
    pub task_id: String,
    /// Current state.
    pub status: TaskStatus,
    /// Optional polling interval.
    pub poll_interval_ms: Option<u64>,
}

/// Bounded cursor-based list result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListResult<T> {
    /// Result page.
    pub items: Vec<T>,
    /// Opaque next cursor.
    pub next_cursor: Option<String>,
}

impl<T> ListResult<T> {
    /// Creates a bounded list page.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when item or cursor bounds are exceeded.
    pub fn new(items: Vec<T>, next_cursor: Option<String>) -> Result<Self, AgentMeshError> {
        if items.len() > MAX_LIST_ITEMS
            || next_cursor.as_ref().is_some_and(|value| value.len() > 4096)
        {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The MCP list result exceeds its configured bounds.",
            ));
        }
        Ok(Self { items, next_cursor })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_tool_payload_round_trips() {
        let params = CallToolParams {
            name: McpName::parse("search.query").unwrap(),
            arguments: BTreeMap::from([("q".into(), Value::String("rust".into()))]),
        };
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(
            serde_json::from_value::<CallToolParams>(value).unwrap(),
            params
        );
    }
    #[test]
    fn list_results_are_bounded() {
        assert!(ListResult::<()>::new(vec![(); MAX_LIST_ITEMS + 1], None).is_err());
    }
}
