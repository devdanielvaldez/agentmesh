//! Allowlists for caller-to-upstream and upstream-to-client headers.

use http::{HeaderMap, HeaderName, header};

const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");
const TRACESTATE: HeaderName = HeaderName::from_static("tracestate");
const BAGGAGE: HeaderName = HeaderName::from_static("baggage");
const MCP_PROTOCOL_VERSION: HeaderName = HeaderName::from_static("mcp-protocol-version");

/// Copies only headers explicitly safe and meaningful for an MCP upstream.
///
/// Caller authorization, cookies, forwarding headers, host, and hop-by-hop
/// headers are intentionally excluded. Upstream credentials are injected later.
pub fn filter_request_headers(source: &HeaderMap) -> HeaderMap {
    copy_allowed(
        source,
        &[
            header::ACCEPT,
            header::CONTENT_TYPE,
            MCP_PROTOCOL_VERSION,
            TRACEPARENT,
            TRACESTATE,
            BAGGAGE,
        ],
    )
}

/// Copies response headers safe to expose through the gateway.
pub fn filter_response_headers(source: &HeaderMap) -> HeaderMap {
    copy_allowed(
        source,
        &[
            header::CONTENT_TYPE,
            header::CACHE_CONTROL,
            header::RETRY_AFTER,
            MCP_PROTOCOL_VERSION,
            TRACEPARENT,
            TRACESTATE,
        ],
    )
}

fn copy_allowed(source: &HeaderMap, allowed: &[HeaderName]) -> HeaderMap {
    let mut destination = HeaderMap::new();
    for name in allowed {
        for value in &source.get_all(name) {
            destination.append(name, value.clone());
        }
    }
    destination
}
