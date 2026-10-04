//! MCP client configuration files for exported capability servers.
//!
//! An exported `server.mjs` is a plain MCP stdio server; every MCP-capable
//! host needs the same facts: the `node` command, the absolute server path,
//! and the Teach session profile that carries logins (without it, tools run
//! in an empty profile). This module builds that document per client flavor
//! and tells the user where the file belongs. Hosts without local-stdio MCP
//! support are deliberately absent: no file can attach such a server to them.

use std::path::{Path, PathBuf};

/// Client flavors with a known local-stdio MCP configuration file.
pub const SUPPORTED_CLIENTS: &[&str] = &["claude-code", "claude-desktop", "generic"];

/// Supported MCP client configuration flavor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClient {
    /// Claude Code (`.mcp.json` in the project root).
    ClaudeCode,
    /// Claude Desktop (`claude_desktop_config.json`).
    ClaudeDesktop,
    /// Any other MCP-capable host that accepts stdio servers.
    Generic,
}

impl McpClient {
    /// Parses a `--client` value; unknown values list what exists.
    ///
    /// # Errors
    ///
    /// Returns [`crate::TeachError::Validation`] naming the supported flavors.
    pub fn parse(name: &str) -> Result<Self, crate::TeachError> {
        match name {
            "claude-code" => Ok(Self::ClaudeCode),
            "claude-desktop" => Ok(Self::ClaudeDesktop),
            "generic" => Ok(Self::Generic),
            other => Err(crate::TeachError::Validation(format!(
                "unknown client {other:?}; use one of: {}",
                SUPPORTED_CLIENTS.join(", ")
            ))),
        }
    }

    /// Suggested file name for the generated document.
    #[must_use]
    pub fn filename(self) -> &'static str {
        match self {
            Self::ClaudeCode => ".mcp.json",
            Self::ClaudeDesktop => "claude_desktop_config.json",
            Self::Generic => "mcp.json",
        }
    }

    /// Where the generated file belongs for this client.
    #[must_use]
    pub fn placement(self) -> &'static str {
        match self {
            Self::ClaudeCode => {
                "copy it to `.mcp.json` in the project root where Claude Code runs \
                 (or merge its `mcpServers` entry into the existing file)"
            }
            Self::ClaudeDesktop => {
                "merge it into `~/Library/Application Support/Claude/claude_desktop_config.json` \
                 (macOS) or `%APPDATA%\\Claude\\claude_desktop_config.json` (Windows), \
                 then restart Claude Desktop"
            }
            Self::Generic => "give this document to any MCP host that accepts local stdio servers",
        }
    }
}

/// Builds the client configuration document for one exported server.
///
/// `server_mjs` must already be absolute; `profile` is the session profile
/// directory whose cookies the tools run with, omitted when tools need no
/// login. The `env` section is left out entirely without a profile.
#[must_use]
pub fn client_config_json(
    server_name: &str,
    server_mjs: &Path,
    profile: Option<&Path>,
) -> serde_json::Value {
    let mut tool = serde_json::Map::new();
    tool.insert(
        "command".to_string(),
        serde_json::Value::String("node".to_string()),
    );
    tool.insert(
        "args".to_string(),
        serde_json::Value::Array(vec![serde_json::Value::String(
            server_mjs.to_string_lossy().into_owned(),
        )]),
    );
    if let Some(profile) = profile {
        tool.insert(
            "env".to_string(),
            serde_json::json!({
                "AGENTMESH_TEACH_PROFILE": profile.to_string_lossy(),
            }),
        );
    }
    let mut servers = serde_json::Map::new();
    servers.insert(server_name.to_string(), serde_json::Value::Object(tool));
    let mut document = serde_json::Map::new();
    document.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
    serde_json::Value::Object(document)
}

/// Server name for the config entry: the sibling `package.json` name when an
/// export directory is given, otherwise a stable fallback.
#[must_use]
pub fn export_server_name(server_mjs: &Path) -> String {
    server_mjs
        .parent()
        .map(|dir| dir.join("package.json"))
        .and_then(|manifest| std::fs::read_to_string(manifest).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|manifest| {
            manifest
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "agentmesh-taught-capabilities".to_string())
}

/// Resolves a `--profile` value to a session profile directory: either a
/// directory path as given, or an application name stored under
/// `<home>/profiles/`. Returns [`None`] when neither exists so callers fail
/// closed instead of writing a config that runs without its login.
#[must_use]
pub fn resolve_client_profile(app_or_path: &str, home: &Path) -> Option<PathBuf> {
    let direct = PathBuf::from(app_or_path);
    if direct.is_dir() {
        return direct.canonicalize().ok();
    }
    let stored = home.join("profiles").join(app_or_path);
    if stored.is_dir() {
        return stored.canonicalize().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn parse_accepts_supported_clients_and_lists_them() {
        assert_eq!(
            McpClient::parse("claude-code").expect("claude-code parses"),
            McpClient::ClaudeCode
        );
        assert_eq!(
            McpClient::parse("claude-desktop").expect("claude-desktop parses"),
            McpClient::ClaudeDesktop
        );
        assert_eq!(
            McpClient::parse("generic").expect("generic parses"),
            McpClient::Generic
        );
        let error = McpClient::parse("chatgpt").expect_err("unknown client must fail");
        let message = error.to_string();
        assert!(
            message.contains("claude-code")
                && message.contains("claude-desktop")
                && message.contains("generic"),
            "error lists supported clients: {message}"
        );
    }

    #[test]
    fn config_carries_command_path_and_profile() {
        let document = client_config_json(
            "demo-mcp",
            Path::new("/opt/demo/server.mjs"),
            Some(Path::new("/home/user/.agentmesh/profiles/demo")),
        );
        let tool = &document["mcpServers"]["demo-mcp"];
        assert_eq!(tool["command"], serde_json::json!("node"));
        assert_eq!(tool["args"], serde_json::json!(["/opt/demo/server.mjs"]));
        assert_eq!(
            tool["env"]["AGENTMESH_TEACH_PROFILE"],
            serde_json::json!("/home/user/.agentmesh/profiles/demo")
        );
    }

    #[test]
    fn config_omits_env_without_profile() {
        let document = client_config_json("demo-mcp", Path::new("/opt/demo/server.mjs"), None);
        let tool = &document["mcpServers"]["demo-mcp"];
        assert_eq!(tool["command"], serde_json::json!("node"));
        assert!(
            tool.get("env").is_none(),
            "no env section without a profile"
        );
    }

    #[test]
    fn server_name_prefers_sibling_manifest() {
        let dir = tempfile::tempdir().expect("scratch dir");
        let server = dir.path().join("server.mjs");
        std::fs::write(&server, "void 0;").expect("scratch server");
        assert_eq!(
            export_server_name(&server),
            "agentmesh-taught-capabilities",
            "missing manifest falls back"
        );
        let mut manifest =
            std::fs::File::create(dir.path().join("package.json")).expect("scratch manifest");
        write!(manifest, "{{\"name\": \"demo-capabilities\"}}").expect("write manifest");
        assert_eq!(export_server_name(&server), "demo-capabilities");
    }

    #[test]
    fn profile_resolves_path_app_and_missing() {
        let home = tempfile::tempdir().expect("scratch home");
        let profile = home.path().join("profiles").join("demo");
        std::fs::create_dir_all(&profile).expect("scratch profile");
        assert_eq!(
            resolve_client_profile(profile.to_str().expect("utf8 path"), home.path()),
            profile.canonicalize().ok(),
            "directory path resolves directly"
        );
        assert_eq!(
            resolve_client_profile("demo", home.path()),
            profile.canonicalize().ok(),
            "app name resolves under profiles/"
        );
        assert_eq!(
            resolve_client_profile("missing", home.path()),
            None,
            "unknown profile fails closed"
        );
    }
}
