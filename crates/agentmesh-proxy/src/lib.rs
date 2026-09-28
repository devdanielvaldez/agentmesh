//! Bounded, credential-safe forwarding to upstream MCP servers.

mod client;
mod credential;
mod endpoint;
mod headers;
mod request;
mod response;

pub use client::{McpProxy, ProxyClient, ProxyConfig, ProxyFuture};
pub use credential::UpstreamCredential;
pub use endpoint::UpstreamEndpoint;
pub use headers::{filter_request_headers, filter_response_headers};
pub use request::ProxyRequest;
pub use response::{ProxyBody, ProxyResponse, ProxyStream};
