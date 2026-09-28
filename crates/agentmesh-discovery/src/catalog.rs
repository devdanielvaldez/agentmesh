//! Pagination, normalization, integrity hashing, and catalog comparison.

use std::collections::{BTreeMap, BTreeSet};

use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_protocol::{
    CapabilitySet, DiscoverResult, McpMethod, ProtocolVersion, SupportedVersions, negotiate_version,
};
use agentmesh_registry::{Capability, CapabilityKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{DiscoveryClient, client::protocol_error};

/// Resource bounds applied across a complete discovery run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryLimits {
    /// Maximum pages across all list methods.
    pub max_pages: usize,
    /// Maximum normalized capabilities across all list methods.
    pub max_capabilities: usize,
    /// Maximum serialized bytes accepted for one advertised capability.
    pub max_capability_bytes: usize,
    /// Maximum bytes accepted in one opaque pagination cursor.
    pub max_cursor_bytes: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            max_pages: 256,
            max_capabilities: 10_000,
            max_capability_bytes: 256 * 1_024,
            max_cursor_bytes: 4_096,
        }
    }
}

impl DiscoveryLimits {
    pub(crate) fn validate(self) -> Result<Self, AgentMeshError> {
        if self.max_pages == 0
            || self.max_capabilities == 0
            || self.max_capability_bytes == 0
            || self.max_cursor_bytes == 0
        {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "Discovery limits must be greater than zero.",
            ));
        }
        Ok(self)
    }
}

/// Cache metadata advertised by one MCP list method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheHint {
    /// Capability family returned by the method.
    pub kind: CapabilityKind,
    /// Server-provided freshness duration.
    pub ttl_ms: Option<u64>,
    /// Server-provided cache visibility scope.
    pub cache_scope: Option<String>,
}

/// Stable key used when comparing catalogs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CapabilityKey {
    /// Capability family.
    pub kind: CapabilityKind,
    /// Exact upstream name or URI.
    pub name: String,
}

/// Complete normalized result of one bounded discovery run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredCatalog {
    /// Protocol revision negotiated through `server/discover`.
    pub protocol_version: ProtocolVersion,
    /// Server capability advertisement used to select list methods.
    pub server_capabilities: CapabilitySet,
    /// Capabilities in deterministic kind-and-name order.
    pub capabilities: Vec<Capability>,
    /// SHA-256 digest over the normalized catalog.
    pub fingerprint: String,
    /// Number of upstream pages consumed.
    pub pages: usize,
    /// Per-family cache hints.
    pub cache_hints: Vec<CacheHint>,
}

/// Added, removed, and metadata-changed capability keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryDiff {
    /// Keys not present in the stored catalog.
    pub added: Vec<CapabilityKey>,
    /// Keys no longer advertised by the upstream.
    pub removed: Vec<CapabilityKey>,
    /// Keys whose normalized metadata or integrity digest changed.
    pub changed: Vec<CapabilityKey>,
}

impl DiscoveryDiff {
    /// Compares a stored catalog with a newly discovered catalog.
    pub fn between(current: &[Capability], discovered: &[Capability]) -> Self {
        let current = index_capabilities(current);
        let discovered = index_capabilities(discovered);
        let added = discovered
            .keys()
            .filter(|key| !current.contains_key(*key))
            .cloned()
            .collect();
        let removed = current
            .keys()
            .filter(|key| !discovered.contains_key(*key))
            .cloned()
            .collect();
        let changed = discovered
            .iter()
            .filter_map(|(key, capability)| {
                current
                    .get(key)
                    .filter(|stored| *stored != capability)
                    .map(|_| key.clone())
            })
            .collect();
        Self {
            added,
            removed,
            changed,
        }
    }

    /// Returns whether the two catalogs are identical.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

struct ListDefinition {
    method: McpMethod,
    result_key: &'static str,
    name_key: &'static str,
    kind: CapabilityKind,
}

const LIST_DEFINITIONS: [ListDefinition; 4] = [
    ListDefinition {
        method: McpMethod::ToolsList,
        result_key: "tools",
        name_key: "name",
        kind: CapabilityKind::Tool,
    },
    ListDefinition {
        method: McpMethod::ResourcesList,
        result_key: "resources",
        name_key: "uri",
        kind: CapabilityKind::Resource,
    },
    ListDefinition {
        method: McpMethod::ResourcesTemplatesList,
        result_key: "resourceTemplates",
        name_key: "uriTemplate",
        kind: CapabilityKind::ResourceTemplate,
    },
    ListDefinition {
        method: McpMethod::PromptsList,
        result_key: "prompts",
        name_key: "name",
        kind: CapabilityKind::Prompt,
    },
];

pub(crate) async fn discover_catalog(
    client: &dyn DiscoveryClient,
    limits: DiscoveryLimits,
) -> Result<DiscoveredCatalog, AgentMeshError> {
    let limits = limits.validate()?;
    let discovery = discover_server(client).await?;
    let server_versions =
        SupportedVersions::new(discovery.supported_versions.clone()).map_err(|error| {
            AgentMeshError::with_source(
                ErrorCode::UpstreamProtocolError,
                "The MCP server did not advertise a protocol version.",
                error,
            )
        })?;
    let protocol_version = negotiate_version(&SupportedVersions::latest_only(), &server_versions)?;
    let mut capabilities = Vec::new();
    let mut keys = BTreeSet::new();
    let mut pages = 0_usize;
    let mut cache_hints = Vec::with_capacity(LIST_DEFINITIONS.len());

    for definition in LIST_DEFINITIONS {
        if !supports_list(&discovery.capabilities, definition.kind) {
            continue;
        }
        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();
        let mut last_hint = CacheHint {
            kind: definition.kind,
            ttl_ms: None,
            cache_scope: None,
        };
        let mut family_pages = 0_usize;
        loop {
            pages = pages.saturating_add(1);
            if pages > limits.max_pages {
                return Err(limit_error("Capability discovery exceeded its page limit."));
            }
            let result = client
                .list_page(definition.method.clone(), cursor.clone())
                .await?;
            let object = result.as_object().ok_or_else(|| {
                protocol_error("An MCP capability list result must be an object.")
            })?;
            let page_ttl = optional_u64(object, "ttlMs")?;
            let page_scope = optional_string(object, "cacheScope")?;
            merge_cache_hint(&mut last_hint, page_ttl, page_scope, family_pages);
            family_pages = family_pages.saturating_add(1);
            let items = object
                .get(definition.result_key)
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    protocol_error("An MCP capability list result is missing its item array.")
                })?;
            for item in items {
                if serde_json::to_vec(item).map_err(encoding_error)?.len()
                    > limits.max_capability_bytes
                {
                    return Err(limit_error(
                        "An advertised MCP capability exceeds its size limit.",
                    ));
                }
                let capability = normalize_capability(item, &definition)?;
                let key = CapabilityKey {
                    kind: capability.kind,
                    name: capability.name.clone(),
                };
                if !keys.insert(key) {
                    return Err(protocol_error(
                        "The MCP server advertised a duplicate capability.",
                    ));
                }
                capabilities.push(capability);
                if capabilities.len() > limits.max_capabilities {
                    return Err(limit_error("Capability discovery exceeded its item limit."));
                }
            }

            let next = optional_string(object, "nextCursor")?;
            let Some(next) = next else {
                break;
            };
            if next.is_empty() || next.len() > limits.max_cursor_bytes {
                return Err(protocol_error("The MCP pagination cursor is invalid."));
            }
            if !seen_cursors.insert(next.clone()) {
                return Err(protocol_error(
                    "The MCP server repeated a pagination cursor.",
                ));
            }
            cursor = Some(next);
        }
        cache_hints.push(last_hint);
    }

    capabilities.sort_by(|left, right| left.kind.cmp(&right.kind).then(left.name.cmp(&right.name)));
    let fingerprint = catalog_fingerprint(&capabilities)?;
    Ok(DiscoveredCatalog {
        protocol_version,
        server_capabilities: discovery.capabilities,
        capabilities,
        fingerprint,
        pages,
        cache_hints,
    })
}

fn merge_cache_hint(
    hint: &mut CacheHint,
    page_ttl: Option<u64>,
    page_scope: Option<String>,
    prior_pages: usize,
) {
    if prior_pages == 0 {
        hint.ttl_ms = page_ttl;
        hint.cache_scope = page_scope;
        return;
    }
    hint.ttl_ms = hint
        .ttl_ms
        .zip(page_ttl)
        .map(|(left, right)| left.min(right));
    if hint.cache_scope != page_scope {
        hint.cache_scope = None;
    }
}

async fn discover_server(client: &dyn DiscoveryClient) -> Result<DiscoverResult, AgentMeshError> {
    let result = client.list_page(McpMethod::ServerDiscover, None).await?;
    serde_json::from_value(result).map_err(|error| {
        AgentMeshError::with_source(
            ErrorCode::UpstreamProtocolError,
            "The MCP server returned an invalid discovery result.",
            error,
        )
    })
}

fn supports_list(capabilities: &CapabilitySet, kind: CapabilityKind) -> bool {
    let name = match kind {
        CapabilityKind::Tool => "tools",
        CapabilityKind::Resource | CapabilityKind::ResourceTemplate => "resources",
        CapabilityKind::Prompt => "prompts",
        CapabilityKind::Task => "io.modelcontextprotocol/tasks",
    };
    capabilities.get(name).is_some()
}

fn normalize_capability(
    item: &Value,
    definition: &ListDefinition,
) -> Result<Capability, AgentMeshError> {
    let object = item
        .as_object()
        .ok_or_else(|| protocol_error("An advertised MCP capability must be an object."))?;
    let name = object
        .get(definition.name_key)
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty() && name.len() <= 1_024)
        .ok_or_else(|| protocol_error("An advertised MCP capability has an invalid name."))?;
    let description = optional_string(object, "description")?;
    let schema = match definition.kind {
        CapabilityKind::Tool => object.get("inputSchema").cloned(),
        CapabilityKind::Prompt => object.get("arguments").cloned(),
        _ => None,
    };
    Ok(Capability {
        kind: definition.kind,
        name: name.into(),
        description,
        schema,
        integrity: Some(value_digest(item)?),
    })
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, AgentMeshError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(protocol_error(
            "An MCP capability list field has an invalid type.",
        )),
    }
}

fn optional_u64(object: &Map<String, Value>, key: &str) -> Result<Option<u64>, AgentMeshError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| protocol_error("An MCP capability list field has an invalid type.")),
        Some(_) => Err(protocol_error(
            "An MCP capability list field has an invalid type.",
        )),
    }
}

fn value_digest(value: &Value) -> Result<String, AgentMeshError> {
    let bytes = serde_json::to_vec(&canonicalize(value)).map_err(encoding_error)?;
    Ok(format_digest(Sha256::digest(bytes)))
}

fn catalog_fingerprint(capabilities: &[Capability]) -> Result<String, AgentMeshError> {
    let bytes = serde_json::to_vec(capabilities).map_err(encoding_error)?;
    Ok(format_digest(Sha256::digest(bytes)))
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let sorted = object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        _ => value.clone(),
    }
}

fn format_digest(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in bytes.as_ref() {
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn index_capabilities(capabilities: &[Capability]) -> BTreeMap<CapabilityKey, &Capability> {
    capabilities
        .iter()
        .map(|capability| {
            (
                CapabilityKey {
                    kind: capability.kind,
                    name: capability.name.clone(),
                },
                capability,
            )
        })
        .collect()
}

fn limit_error(message: &'static str) -> AgentMeshError {
    AgentMeshError::new(ErrorCode::PayloadTooLarge, message)
}

fn encoding_error(source: serde_json::Error) -> AgentMeshError {
    AgentMeshError::with_source(
        ErrorCode::UpstreamProtocolError,
        "The MCP capability catalog could not be normalized.",
        source,
    )
}
