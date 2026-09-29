//! Multi-upstream fan-out with discovery-based routing.
//!
//! When the gateway is configured with several static upstreams, this proxy
//! aggregates the catalog families (`tools/list`, `resources/list`,
//! `resources/templates/list`, `prompts/list`) across every upstream and
//! routes keyed calls (`tools/call`, `prompts/get`, `resources/read` and the
//! resource subscription methods) to the upstream that advertised the
//! capability during startup discovery. `ping` fans out to all upstreams and
//! notifications are broadcast. Any other method requires a single upstream
//! and is rejected explicitly so callers never observe silent partial routing.
//!
//! `tools/list` is mandatory per upstream; the remaining families are
//! optional, because most real servers implement none of them (bridges answer
//! those methods with HTTP 404 and "method not found"). List merges only query
//! targets that advertised the family, so absent families merge as empty
//! instead of failing the request.

use std::{collections::HashMap, sync::RwLock, time::Duration};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{
    CLIENT_CAPABILITIES_KEY, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse,
    LATEST_PROTOCOL_VERSION, McpMethod, PROTOCOL_VERSION_KEY, RequestId,
};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use serde_json::Value;

use crate::{
    McpProxy, ProxyBody, ProxyClient, ProxyFuture, ProxyRequest, ProxyResponse, UpstreamCredential,
    UpstreamEndpoint,
};

/// JSON-RPC "method not found": the upstream does not offer that catalog family.
const METHOD_NOT_FOUND: i32 = -32_601;
/// Merge bound matching the protocol list-item bound.
const MERGED_LIST_LIMIT: usize = 10_000;

/// One static fan-out target with its own deadline.
#[derive(Debug, Clone)]
pub struct UpstreamTarget {
    /// Validated Streamable HTTP MCP endpoint.
    pub endpoint: UpstreamEndpoint,
    /// Per-request deadline applied to this target.
    pub timeout: Duration,
}

/// Catalog families a target advertised at discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CatalogFamily {
    /// Upstream answered `tools/list`.
    Tools,
    /// Upstream answered `resources/list`.
    Resources,
    /// Upstream answered `resources/templates/list`.
    Templates,
    /// Upstream answered `prompts/list`.
    Prompts,
}

/// Discovery-built capability tables mapping names to target indexes.
#[derive(Debug, Default)]
struct DiscoveryTables {
    /// Tool name to target index.
    tools: HashMap<String, usize>,
    /// Resource URI to target index.
    resources: HashMap<String, usize>,
    /// Prompt name to target index.
    prompts: HashMap<String, usize>,
    /// Advertised families per target, in target order.
    support: Vec<Vec<CatalogFamily>>,
}

/// Fan-out proxy over several static upstreams.
///
/// Call [`MultiUpstreamProxy::initialize`] once before serving traffic; it
/// fails closed when any upstream cannot be discovered.
pub struct MultiUpstreamProxy {
    proxy: ProxyClient,
    targets: Vec<UpstreamTarget>,
    tables: RwLock<DiscoveryTables>,
}

impl MultiUpstreamProxy {
    /// Creates a fan-out proxy over at least one target.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCode::ConfigurationInvalid`] when no target is given.
    pub fn new(proxy: ProxyClient, targets: Vec<UpstreamTarget>) -> Result<Self, AgentMeshError> {
        if targets.is_empty() {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "Multi-upstream fan-out requires at least one target.",
            ));
        }
        Ok(Self {
            proxy,
            targets,
            tables: RwLock::new(DiscoveryTables::default()),
        })
    }

    /// Returns the number of configured targets.
    #[must_use]
    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    /// Discovers catalogs on every target and builds the routing tables.
    ///
    /// `tools/list` is mandatory: its failure aborts initialization so the
    /// gateway never serves a silently partial mesh. The remaining families
    /// are optional per upstream — most real servers implement none of them,
    /// and bridges answer HTTP 404 with "method not found" — so a failed
    /// family query only marks that family absent for that upstream.
    ///
    /// # Errors
    ///
    /// Returns the first upstream or protocol failure encountered.
    pub async fn initialize(&self) -> Result<(), AgentMeshError> {
        let mut tables = DiscoveryTables::default();
        for (index, target) in self.targets.iter().enumerate() {
            let mut support = Vec::with_capacity(4);
            let tool_items = self
                .fetch_list(target, McpMethod::ToolsList, "tools")
                .await
                .map_err(|error| {
                    AgentMeshError::with_source(
                        ErrorCode::UpstreamUnavailable,
                        format!(
                            "Upstream #{index} tools/list discovery failed; refusing partial mesh."
                        ),
                        error,
                    )
                })?;
            support.push(CatalogFamily::Tools);
            index_names(&mut tables.tools, &tool_items, "name", index);
            for (method, family, key, name_key) in [
                (
                    McpMethod::ResourcesList,
                    CatalogFamily::Resources,
                    "resources",
                    "uri",
                ),
                (
                    McpMethod::ResourcesTemplatesList,
                    CatalogFamily::Templates,
                    "resourceTemplates",
                    "uri",
                ),
                (
                    McpMethod::PromptsList,
                    CatalogFamily::Prompts,
                    "prompts",
                    "name",
                ),
            ] {
                match self.fetch_list(target, method, key).await {
                    Ok(items) => {
                        support.push(family);
                        let table = match family {
                            CatalogFamily::Resources => &mut tables.resources,
                            CatalogFamily::Prompts => &mut tables.prompts,
                            CatalogFamily::Tools | CatalogFamily::Templates => continue,
                        };
                        index_names(table, &items, name_key, index);
                    }
                    Err(error) => {
                        tracing::warn!(
                            upstream = index,
                            key,
                            error = %error,
                            "Catalog family unavailable; treating it as absent for this upstream."
                        );
                    }
                }
            }
            tables.support.push(support);
        }
        *self.tables.write().map_err(|_| {
            AgentMeshError::new(
                ErrorCode::ConfigurationUnavailable,
                "The fan-out routing tables are unavailable.",
            )
        })? = tables;
        Ok(())
    }

    /// Fetches one catalog page; "method not found" means the family is absent.
    async fn fetch_list(
        &self,
        target: &UpstreamTarget,
        method: McpMethod,
        key: &str,
    ) -> Result<Vec<Value>, AgentMeshError> {
        let outgoing = ProxyRequest::new(target.endpoint.clone(), discovery_message(method))
            .with_headers(discovery_headers()?)
            .with_timeout(target.timeout);
        let (_, _, body) = self.proxy.execute(outgoing).await?.into_parts();
        match body {
            ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::Success(success))) => {
                reject_paged_list(&success.result)?;
                extract_list_items(&success.result, key)
            }
            ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::Error(failure)))
                if failure.error.code == METHOD_NOT_FOUND =>
            {
                Ok(Vec::new())
            }
            ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::Error(_))) => {
                Err(AgentMeshError::new(
                    ErrorCode::UpstreamProtocolError,
                    "An upstream catalog query returned an error.",
                ))
            }
            _ => Err(AgentMeshError::new(
                ErrorCode::UpstreamProtocolError,
                "An upstream catalog query returned an unexpected message.",
            )),
        }
    }

    /// Executes one validated gateway message without requiring a request endpoint.
    ///
    /// # Errors
    ///
    /// Returns routing, upstream, or protocol failures.
    pub async fn execute_message(
        &self,
        message: JsonRpcMessage,
        headers: HeaderMap,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let request =
            ProxyRequest::new(self.targets[0].endpoint.clone(), message).with_headers(headers);
        self.execute_inner(request).await
    }

    /// Executes one validated gateway message across the fan-out targets.
    async fn execute_inner(&self, request: ProxyRequest) -> Result<ProxyResponse, AgentMeshError> {
        let ProxyRequest {
            message,
            headers,
            credential,
            ..
        } = request;
        match message {
            JsonRpcMessage::Request(request) => {
                self.dispatch_request(request, headers, credential).await
            }
            JsonRpcMessage::Notification(notification) => {
                let message = JsonRpcMessage::Notification(notification);
                self.broadcast(message, headers, credential).await
            }
            JsonRpcMessage::Response(_) => Err(AgentMeshError::new(
                ErrorCode::InvalidRequest,
                "An MCP response cannot be forwarded as an upstream request.",
            )),
        }
    }

    /// Dispatches one validated MCP request across the fan-out targets.
    async fn dispatch_request(
        &self,
        request: JsonRpcRequest,
        headers: HeaderMap,
        credential: Option<UpstreamCredential>,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let params = request.params.clone();
        let message = JsonRpcMessage::Request(request);
        match param_method(&message) {
            McpMethod::ToolsCall => {
                self.route_call(
                    message,
                    headers,
                    credential,
                    "tool",
                    param_name(params.as_ref(), "name"),
                )
                .await
            }
            McpMethod::PromptsGet => {
                self.route_call(
                    message,
                    headers,
                    credential,
                    "prompt",
                    param_name(params.as_ref(), "name"),
                )
                .await
            }
            McpMethod::ResourcesRead
            | McpMethod::ResourcesSubscribe
            | McpMethod::ResourcesUnsubscribe => {
                self.route_call(
                    message,
                    headers,
                    credential,
                    "resource",
                    param_name(params.as_ref(), "uri"),
                )
                .await
            }
            McpMethod::ToolsList => self.merge_lists(message, headers, "tools").await,
            McpMethod::ResourcesList => self.merge_lists(message, headers, "resources").await,
            McpMethod::ResourcesTemplatesList => {
                self.merge_lists(message, headers, "resourceTemplates")
                    .await
            }
            McpMethod::PromptsList => self.merge_lists(message, headers, "prompts").await,
            McpMethod::Ping => self.fanout_ping(message, headers).await,
            other => Err(AgentMeshError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Method {} cannot be routed across {} upstreams; configure a single upstream.",
                    other.as_str(),
                    self.targets.len()
                ),
            )),
        }
    }

    /// Routes a keyed call to the discovered owner.
    async fn route_call(
        &self,
        message: JsonRpcMessage,
        headers: HeaderMap,
        credential: Option<UpstreamCredential>,
        kind: &str,
        key: Option<&str>,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let name = key.ok_or_else(|| {
            AgentMeshError::new(
                ErrorCode::InvalidRequest,
                format!("The {kind} call is missing its routing key."),
            )
        })?;
        let index = {
            let tables = self.tables.read().map_err(|_| {
                AgentMeshError::new(
                    ErrorCode::ConfigurationUnavailable,
                    "The fan-out routing tables are unavailable.",
                )
            })?;
            let table = match kind {
                "tool" => &tables.tools,
                "prompt" => &tables.prompts,
                _ => &tables.resources,
            };
            *table.get(name).ok_or_else(|| {
                AgentMeshError::new(
                    ErrorCode::CapabilityNotFound,
                    format!("Unknown {kind}: {name}."),
                )
            })?
        };
        let target = &self.targets[index];
        let mut outgoing = ProxyRequest::new(target.endpoint.clone(), message)
            .with_headers(headers)
            .with_timeout(target.timeout);
        if let Some(credential) = credential {
            outgoing = outgoing.with_credential(credential);
        }
        self.proxy.execute(outgoing).await
    }

    /// Fans out a catalog list and merges the raw pages.
    ///
    /// Only targets that advertised the family at discovery are queried; the
    /// rest contribute nothing. A supporting target that fails now propagates
    /// its failure instead of silently shrinking the catalog.
    async fn merge_lists(
        &self,
        message: JsonRpcMessage,
        headers: HeaderMap,
        key: &str,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let id = request_id(&message);
        let eligible: Vec<(usize, &UpstreamTarget)> = {
            let tables = self.tables.read().map_err(|_| {
                AgentMeshError::new(
                    ErrorCode::ConfigurationUnavailable,
                    "The fan-out routing tables are unavailable.",
                )
            })?;
            self.targets
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    tables
                        .support
                        .get(*index)
                        .is_some_and(|support| family_supported(support, key))
                })
                .collect()
        };
        let mut merged = Vec::new();
        for (_, target) in eligible {
            let outgoing = ProxyRequest::new(target.endpoint.clone(), message.clone())
                .with_headers(json_accept(&headers))
                .with_timeout(target.timeout);
            let (_, _, body) = self.proxy.execute(outgoing).await?.into_parts();
            match body {
                ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::Success(success))) => {
                    reject_paged_list(&success.result)?;
                    merged.extend(extract_list_items(&success.result, key)?);
                }
                ProxyBody::Json(JsonRpcMessage::Response(response)) if response.is_error() => {
                    if error_code(&response) == Some(METHOD_NOT_FOUND) {
                        continue;
                    }
                    return Ok(ProxyResponse::new(
                        StatusCode::OK,
                        HeaderMap::new(),
                        ProxyBody::Json(JsonRpcMessage::Response(response)),
                    ));
                }
                _ => {
                    return Err(AgentMeshError::new(
                        ErrorCode::UpstreamProtocolError,
                        "An upstream list query returned an unexpected message.",
                    ));
                }
            }
        }
        if merged.len() > MERGED_LIST_LIMIT {
            return Err(AgentMeshError::new(
                ErrorCode::PayloadTooLarge,
                "The merged upstream catalog exceeds its size bound.",
            ));
        }
        let mut result = serde_json::Map::with_capacity(1);
        result.insert(key.to_string(), Value::Array(merged));
        Ok(ProxyResponse::new(
            StatusCode::OK,
            HeaderMap::new(),
            ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::success(
                id,
                Value::Object(result),
            ))),
        ))
    }

    /// Fans out `ping`; every upstream must answer successfully.
    async fn fanout_ping(
        &self,
        message: JsonRpcMessage,
        headers: HeaderMap,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let mut first: Option<ProxyResponse> = None;
        for target in &self.targets {
            let outgoing = ProxyRequest::new(target.endpoint.clone(), message.clone())
                .with_headers(headers.clone())
                .with_timeout(target.timeout);
            let (status, headers, body) = self.proxy.execute(outgoing).await?.into_parts();
            if is_error_envelope(&body) {
                return Ok(ProxyResponse::new(status, headers, body));
            }
            if first.is_none() {
                first = Some(ProxyResponse::new(status, headers, body));
            }
        }
        first.ok_or_else(|| AgentMeshError::new(ErrorCode::Internal, "Fan-out has no targets."))
    }

    /// Broadcasts a notification to every target; all must accept it.
    async fn broadcast(
        &self,
        message: JsonRpcMessage,
        headers: HeaderMap,
        credential: Option<UpstreamCredential>,
    ) -> Result<ProxyResponse, AgentMeshError> {
        let method = param_method(&message).as_str().to_string();
        for target in &self.targets {
            let mut outgoing = ProxyRequest::new(target.endpoint.clone(), message.clone())
                .with_headers(headers.clone())
                .with_timeout(target.timeout);
            if let Some(credential) = credential.clone() {
                outgoing = outgoing.with_credential(credential);
            }
            let (_, _, body) = self.proxy.execute(outgoing).await?.into_parts();
            if !matches!(body, ProxyBody::Empty) {
                return Err(AgentMeshError::new(
                    ErrorCode::UpstreamProtocolError,
                    format!("Upstream rejected broadcast notification {method}."),
                ));
            }
        }
        Ok(ProxyResponse::new(
            StatusCode::ACCEPTED,
            HeaderMap::new(),
            ProxyBody::Empty,
        ))
    }
}

impl McpProxy for MultiUpstreamProxy {
    fn execute(&self, request: ProxyRequest) -> ProxyFuture<'_> {
        Box::pin(async move { self.execute_inner(request).await })
    }
}

/// Builds the internal catalog discovery request.
fn discovery_message(method: McpMethod) -> JsonRpcMessage {
    let mut meta = serde_json::Map::with_capacity(2);
    meta.insert(
        PROTOCOL_VERSION_KEY.to_string(),
        Value::String(LATEST_PROTOCOL_VERSION.to_string()),
    );
    meta.insert(
        CLIENT_CAPABILITIES_KEY.to_string(),
        Value::Object(serde_json::Map::new()),
    );
    let mut params = serde_json::Map::with_capacity(1);
    params.insert("_meta".to_string(), Value::Object(meta));
    JsonRpcMessage::Request(JsonRpcRequest::new(
        RequestId::Integer(0),
        method,
        Some(Value::Object(params)),
    ))
}

/// Protocol version header carried by internal discovery requests.
const MCP_PROTOCOL_VERSION_HEADER: HeaderName = HeaderName::from_static("mcp-protocol-version");

/// Builds headers for internal discovery requests.
///
/// Includes the Streamable HTTP `Accept` pair: strict servers answer 406
/// without it, which would fail closed startup discovery.
fn discovery_headers() -> Result<HeaderMap, AgentMeshError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert(
        MCP_PROTOCOL_VERSION_HEADER,
        HeaderValue::from_str(LATEST_PROTOCOL_VERSION).map_err(|_| {
            AgentMeshError::new(
                ErrorCode::Internal,
                "The protocol version cannot be represented as a header.",
            )
        })?,
    );
    Ok(headers)
}

/// Forces JSON list responses so pages can be merged.
fn json_accept(source: &HeaderMap) -> HeaderMap {
    let mut headers = source.clone();
    headers.insert(
        http::header::ACCEPT,
        HeaderValue::from_static("application/json"),
    );
    headers
}

/// Indexes catalog names into a routing table; first upstream wins on conflict.
fn index_names(
    table: &mut HashMap<String, usize>,
    items: &[Value],
    name_key: &str,
    index: usize,
) {
    for item in items {
        let Some(name) = item.get(name_key).and_then(Value::as_str) else {
            tracing::warn!(upstream = index, "Skipping unnamed catalog entry.");
            continue;
        };
        if let Some(previous) = table.insert(name.to_string(), index) {
            if previous != index {
                tracing::warn!(
                    name,
                    kept = previous,
                    dropped = index,
                    "Duplicate capability across upstreams; first upstream wins."
                );
            }
        }
    }
}

/// Reports whether a target advertised a merged list family at discovery.
fn family_supported(support: &[CatalogFamily], key: &str) -> bool {
    let family = match key {
        "tools" => CatalogFamily::Tools,
        "resources" => CatalogFamily::Resources,
        "resourceTemplates" => CatalogFamily::Templates,
        _ => CatalogFamily::Prompts,
    };
    support.contains(&family)
}

/// Extracts the MCP method from any message shape.
fn param_method(message: &JsonRpcMessage) -> McpMethod {
    match message {
        JsonRpcMessage::Request(request) => request.method.clone(),
        JsonRpcMessage::Notification(notification) => notification.method.clone(),
        JsonRpcMessage::Response(_) => McpMethod::from_wire("response"),
    }
}

/// Extracts the routing key from call parameters.
fn param_name<'a>(params: Option<&'a Value>, key: &str) -> Option<&'a str> {
    params?.get(key)?.as_str()
}

/// Extracts the request identifier; only requests reach this proxy.
fn request_id(message: &JsonRpcMessage) -> RequestId {
    match message {
        JsonRpcMessage::Request(request) => request.id.clone(),
        JsonRpcMessage::Notification(_) | JsonRpcMessage::Response(_) => RequestId::Integer(0),
    }
}

/// Reports whether a proxy body carries a JSON-RPC failure envelope.
fn is_error_envelope(body: &ProxyBody) -> bool {
    matches!(
        body,
        ProxyBody::Json(JsonRpcMessage::Response(response)) if response.is_error()
    )
}

/// Reads the JSON-RPC error code when the response is a failure.
fn error_code(response: &JsonRpcResponse) -> Option<i32> {
    match response {
        JsonRpcResponse::Error(failure) => Some(failure.error.code),
        JsonRpcResponse::Success(_) => None,
    }
}

/// Extracts the raw catalog array for a list key.
fn extract_list_items(result: &Value, key: &str) -> Result<Vec<Value>, AgentMeshError> {
    match result.get(key) {
        Some(Value::Array(items)) => Ok(items.clone()),
        _ => Err(AgentMeshError::new(
            ErrorCode::UpstreamProtocolError,
            "An upstream list response has an unexpected shape.",
        )),
    }
}

/// Rejects paginated catalogs, which cannot merge losslessly across upstreams.
fn reject_paged_list(result: &Value) -> Result<(), AgentMeshError> {
    let paged = result
        .get("nextCursor")
        .is_some_and(|cursor| !cursor.is_null());
    if paged {
        return Err(AgentMeshError::new(
            ErrorCode::UpstreamProtocolError,
            "Paginated upstream catalogs cannot merge across upstreams.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProxyConfig;
    use axum::{Json, Router, extract::State, routing::post};
    use serde_json::json;
    use tokio::{net::TcpListener, task::JoinHandle};

    #[derive(Clone)]
    struct FakeServer {
        tools: Vec<(String, String)>,
        /// When false, non-tool families answer HTTP 404 like real bridges.
        full_catalog: bool,
    }

    async fn handler(
        State(state): State<FakeServer>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body.get("method").and_then(Value::as_str).unwrap_or("");
        if !state.full_catalog
            && matches!(
                method,
                "resources/list" | "resources/templates/list" | "prompts/list"
            )
        {
            return (
                StatusCode::NOT_FOUND,
                Json(
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32_601, "message": "Method not found"}}),
                ),
            );
        }
        let result = match method {
            "tools/list" => json!({"tools": state
                .tools
                .iter()
                .map(|(name, description)| json!({
                    "name": name,
                    "description": description,
                    "inputSchema": {"type": "object"},
                }))
                .collect::<Vec<_>>()}),
            "tools/call" => {
                let name = body
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = state
                    .tools
                    .iter()
                    .find(|(tool, _)| tool == name)
                    .map_or("unknown", |(_, text)| text);
                json!({"content": [{"type": "text", "text": text}]})
            }
            "resources/list" => json!({"resources": []}),
            "prompts/list" => json!({"prompts": []}),
            "ping" => json!({}),
            _ => {
                return (
                    StatusCode::OK,
                    Json(
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32_601, "message": "no such method"}}),
                    ),
                );
            }
        };
        (
            StatusCode::OK,
            Json(json!({"jsonrpc": "2.0", "id": id, "result": result})),
        )
    }

    async fn start_fake(
        tools: Vec<(&str, &str)>,
        full_catalog: bool,
    ) -> (UpstreamTarget, JoinHandle<()>) {
        let state = FakeServer {
            tools: tools
                .into_iter()
                .map(|(name, text)| (name.to_string(), text.to_string()))
                .collect(),
            full_catalog,
        };
        let app = Router::new().route("/mcp", post(handler)).with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener binds");
        let port = listener.local_addr().expect("listener address").port();
        let handle =
            tokio::spawn(async move { axum::serve(listener, app).await.expect("test server") });
        let target = UpstreamTarget {
            endpoint: UpstreamEndpoint::parse(&format!("http://127.0.0.1:{port}/mcp"), true)
                .expect("test endpoint"),
            timeout: Duration::from_secs(10),
        };
        (target, handle)
    }

    async fn fanout(
        tools: Vec<(Vec<(&str, &str)>, bool)>,
    ) -> (MultiUpstreamProxy, Vec<JoinHandle<()>>) {
        let mut targets = Vec::new();
        let mut handles = Vec::new();
        for (server_tools, full_catalog) in tools {
            let (target, handle) = start_fake(server_tools, full_catalog).await;
            targets.push(target);
            handles.push(handle);
        }
        let proxy = ProxyClient::new(ProxyConfig::default()).expect("proxy client");
        let fanout = MultiUpstreamProxy::new(proxy, targets).expect("fan-out proxy");
        fanout.initialize().await.expect("discovery succeeds");
        (fanout, handles)
    }

    #[test]
    fn constructor_rejects_empty_targets() {
        let proxy = ProxyClient::new(ProxyConfig::default()).expect("proxy client");
        assert!(MultiUpstreamProxy::new(proxy, Vec::new()).is_err());
    }

    #[test]
    fn discovery_headers_carry_version_and_accept() {
        let headers = discovery_headers().expect("discovery headers");
        assert_eq!(
            headers.get(http::header::ACCEPT).expect("accept header"),
            "application/json, text/event-stream"
        );
        assert!(headers.contains_key(MCP_PROTOCOL_VERSION_HEADER));
    }

    #[tokio::test]
    async fn lists_merge_and_calls_route_to_owners() {
        let (fanout, handles) = fanout(vec![
            (vec![("add", "calc says 12")], true),
            (vec![("note_list", "notes say hi")], true),
        ])
        .await;

        let listed = fanout
            .execute_message(discovery_like("tools/list"), HeaderMap::new())
            .await
            .expect("merged list");
        let names = list_names(listed, "tools");
        assert!(names.contains(&"add".to_string()));
        assert!(names.contains(&"note_list".to_string()));

        let called = fanout
            .execute_message(
                call_like("note_list", serde_json::Map::new()),
                HeaderMap::new(),
            )
            .await
            .expect("routed call");
        assert!(response_text(called).contains("notes say hi"));

        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let (fanout, handles) = fanout(vec![(vec![("add", "calc says 12")], true)]).await;
        let error = fanout
            .execute_message(
                call_like("missing", serde_json::Map::new()),
                HeaderMap::new(),
            )
            .await
            .expect_err("unknown tool fails");
        assert_eq!(error.code(), ErrorCode::CapabilityNotFound);
        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn non_routable_method_is_rejected() {
        let (fanout, handles) = fanout(vec![(vec![("add", "calc says 12")], true)]).await;
        let error = fanout
            .execute_message(discovery_like("server/discover"), HeaderMap::new())
            .await
            .expect_err("discover is not routable");
        assert_eq!(error.code(), ErrorCode::InvalidRequest);
        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn ping_fans_out_to_every_target() {
        let (fanout, handles) = fanout(vec![
            (vec![("add", "calc says 12")], true),
            (vec![("note_list", "notes say hi")], true),
        ])
        .await;
        assert_eq!(fanout.target_count(), 2);
        fanout
            .execute_message(discovery_like("ping"), HeaderMap::new())
            .await
            .expect("ping succeeds");
        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn tools_only_upstream_initializes_and_merges_empty_catalogs() {
        // Regression: real servers answer non-tool families with HTTP 404,
        // which must mark the family absent instead of failing discovery.
        let (fanout, handles) = fanout(vec![(vec![("add", "calc says 12")], false)]).await;

        let listed = fanout
            .execute_message(discovery_like("tools/list"), HeaderMap::new())
            .await
            .expect("merged tools");
        assert!(list_names(listed, "tools").contains(&"add".to_string()));

        for method in ["resources/list", "resources/templates/list", "prompts/list"] {
            let merged = fanout
                .execute_message(discovery_like(method), HeaderMap::new())
                .await
                .unwrap_or_else(|_| panic!("{method} merges as empty"));
            let key = match method {
                "resources/list" => "resources",
                "resources/templates/list" => "resourceTemplates",
                _ => "prompts",
            };
            assert!(list_names(merged, key).is_empty());
        }

        let called = fanout
            .execute_message(
                call_like("add", serde_json::Map::new()),
                HeaderMap::new(),
            )
            .await
            .expect("routed call");
        assert!(response_text(called).contains("calc says 12"));

        for handle in handles {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn dead_upstream_still_fails_initialization() {
        // tools/list stays strict: an unreachable server fails closed.
        let proxy = ProxyClient::new(ProxyConfig::default()).expect("proxy client");
        let target = UpstreamTarget {
            endpoint: UpstreamEndpoint::parse("http://127.0.0.1:1/mcp", true)
                .expect("test endpoint"),
            timeout: Duration::from_secs(2),
        };
        let fanout = MultiUpstreamProxy::new(proxy, vec![target]).expect("fan-out proxy");
        assert!(fanout.initialize().await.is_err());
    }

    /// Builds a gateway-shaped request without transport headers.
    fn discovery_like(method: &str) -> JsonRpcMessage {
        JsonRpcMessage::Request(JsonRpcRequest::new(
            RequestId::Integer(1),
            McpMethod::from_wire(method),
            Some(Value::Object(serde_json::Map::new())),
        ))
    }

    /// Builds a gateway-shaped `tools/call` request.
    fn call_like(tool: &str, arguments: serde_json::Map<String, Value>) -> JsonRpcMessage {
        let mut params = serde_json::Map::with_capacity(2);
        params.insert("name".to_string(), Value::String(tool.to_string()));
        params.insert("arguments".to_string(), Value::Object(arguments));
        JsonRpcMessage::Request(JsonRpcRequest::new(
            RequestId::Integer(2),
            McpMethod::ToolsCall,
            Some(Value::Object(params)),
        ))
    }

    /// Extracts merged catalog names from a fan-out response.
    fn list_names(response: ProxyResponse, key: &str) -> Vec<String> {
        let (_, _, body) = response.into_parts();
        match body {
            ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::Success(success))) => success
                .result
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("name")?.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Extracts call text from a fan-out response.
    fn response_text(response: ProxyResponse) -> String {
        let (_, _, body) = response.into_parts();
        match body {
            ProxyBody::Json(JsonRpcMessage::Response(JsonRpcResponse::Success(success))) => success
                .result
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        }
    }
}
