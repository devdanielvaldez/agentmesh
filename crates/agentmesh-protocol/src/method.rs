//! MCP method names with forward-compatible extension handling.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Known MCP methods plus unknown future or vendor-defined methods.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum McpMethod {
    /// Modern server capability discovery.
    ServerDiscover,
    /// Connectivity check.
    Ping,
    /// Lists tools.
    ToolsList,
    /// Calls a tool.
    ToolsCall,
    /// Lists resources.
    ResourcesList,
    /// Lists resource templates.
    ResourcesTemplatesList,
    /// Reads a resource.
    ResourcesRead,
    /// Subscribes to a resource.
    ResourcesSubscribe,
    /// Removes a resource subscription.
    ResourcesUnsubscribe,
    /// Lists prompts.
    PromptsList,
    /// Retrieves a prompt.
    PromptsGet,
    /// Requests argument or reference completion.
    CompletionComplete,
    /// Changes the requested server logging level.
    LoggingSetLevel,
    /// Gets a long-running task.
    TasksGet,
    /// Retrieves a completed task result.
    TasksResult,
    /// Cancels a long-running task.
    TasksCancel,
    /// Lists long-running tasks.
    TasksList,
    /// Legacy connection initialization.
    Initialize,
    /// Legacy initialization completion notification.
    Initialized,
    /// Request cancellation notification.
    Cancelled,
    /// Progress notification.
    Progress,
    /// Logging message notification.
    LoggingMessage,
    /// Tool catalog change notification.
    ToolsListChanged,
    /// Resource catalog change notification.
    ResourcesListChanged,
    /// Resource content change notification.
    ResourcesUpdated,
    /// Prompt catalog change notification.
    PromptsListChanged,
    /// A future or vendor-defined method preserved verbatim.
    Other(String),
}

impl McpMethod {
    /// Parses a wire method while preserving unknown extensions.
    pub fn from_wire(value: impl Into<String>) -> Self {
        let value = value.into();
        match value.as_str() {
            "server/discover" => Self::ServerDiscover,
            "ping" => Self::Ping,
            "tools/list" => Self::ToolsList,
            "tools/call" => Self::ToolsCall,
            "resources/list" => Self::ResourcesList,
            "resources/templates/list" => Self::ResourcesTemplatesList,
            "resources/read" => Self::ResourcesRead,
            "resources/subscribe" => Self::ResourcesSubscribe,
            "resources/unsubscribe" => Self::ResourcesUnsubscribe,
            "prompts/list" => Self::PromptsList,
            "prompts/get" => Self::PromptsGet,
            "completion/complete" => Self::CompletionComplete,
            "logging/setLevel" => Self::LoggingSetLevel,
            "tasks/get" => Self::TasksGet,
            "tasks/result" => Self::TasksResult,
            "tasks/cancel" => Self::TasksCancel,
            "tasks/list" => Self::TasksList,
            "initialize" => Self::Initialize,
            "notifications/initialized" => Self::Initialized,
            "notifications/cancelled" => Self::Cancelled,
            "notifications/progress" => Self::Progress,
            "notifications/message" => Self::LoggingMessage,
            "notifications/tools/list_changed" => Self::ToolsListChanged,
            "notifications/resources/list_changed" => Self::ResourcesListChanged,
            "notifications/resources/updated" => Self::ResourcesUpdated,
            "notifications/prompts/list_changed" => Self::PromptsListChanged,
            _ => Self::Other(value),
        }
    }

    /// Returns the exact wire method.
    pub fn as_str(&self) -> &str {
        match self {
            Self::ServerDiscover => "server/discover",
            Self::Ping => "ping",
            Self::ToolsList => "tools/list",
            Self::ToolsCall => "tools/call",
            Self::ResourcesList => "resources/list",
            Self::ResourcesTemplatesList => "resources/templates/list",
            Self::ResourcesRead => "resources/read",
            Self::ResourcesSubscribe => "resources/subscribe",
            Self::ResourcesUnsubscribe => "resources/unsubscribe",
            Self::PromptsList => "prompts/list",
            Self::PromptsGet => "prompts/get",
            Self::CompletionComplete => "completion/complete",
            Self::LoggingSetLevel => "logging/setLevel",
            Self::TasksGet => "tasks/get",
            Self::TasksResult => "tasks/result",
            Self::TasksCancel => "tasks/cancel",
            Self::TasksList => "tasks/list",
            Self::Initialize => "initialize",
            Self::Initialized => "notifications/initialized",
            Self::Cancelled => "notifications/cancelled",
            Self::Progress => "notifications/progress",
            Self::LoggingMessage => "notifications/message",
            Self::ToolsListChanged => "notifications/tools/list_changed",
            Self::ResourcesListChanged => "notifications/resources/list_changed",
            Self::ResourcesUpdated => "notifications/resources/updated",
            Self::PromptsListChanged => "notifications/prompts/list_changed",
            Self::Other(value) => value,
        }
    }

    /// Returns whether the method belongs to the legacy connection handshake.
    pub const fn is_legacy_lifecycle(&self) -> bool {
        matches!(self, Self::Initialize | Self::Initialized)
    }
}

impl fmt::Display for McpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for McpMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for McpMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_wire)
    }
}
