//! Authenticated administrative API with optimistic concurrency and tenant scoping.

use agentmesh_control_plane::{ConfigSnapshot, ControlPlane, DesiredResource};
use agentmesh_error::{AgentMeshError, ErrorCode};
use agentmesh_storage::{DocumentStore, StorageScope, WriteCondition};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// API limits used to bound administrative requests.
#[derive(Debug, Clone, Copy)]
pub struct ApiLimits {
    /// Maximum accepted JSON bytes.
    pub max_body_bytes: usize,
}
impl Default for ApiLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 1024 * 1024,
        }
    }
}

/// Hashed bearer-token verifier. The plaintext token is never stored.
#[derive(Clone)]
pub struct AdminToken([u8; 32]);
impl std::fmt::Debug for AdminToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminToken([REDACTED])")
    }
}
impl AdminToken {
    /// Hashes a non-empty bootstrap token.
    ///
    /// # Errors
    ///
    /// Returns [`AgentMeshError`] when the token is shorter than 16 bytes.
    pub fn new(token: &str) -> Result<Self, AgentMeshError> {
        if token.len() < 16 {
            return Err(AgentMeshError::new(
                ErrorCode::ConfigurationInvalid,
                "The administrative token must contain at least 16 bytes.",
            ));
        }
        Ok(Self(Sha256::digest(token.as_bytes()).into()))
    }
    fn permits(&self, headers: &HeaderMap) -> bool {
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|token| constant_time_eq(&self.0, &Sha256::digest(token.as_bytes())))
    }
}

struct ApiState<S> {
    plane: Arc<ControlPlane<S>>,
    token: AdminToken,
}

impl<S> Clone for ApiState<S> {
    fn clone(&self) -> Self {
        Self {
            plane: Arc::clone(&self.plane),
            token: self.token.clone(),
        }
    }
}

/// Builds the versioned administrative router.
pub fn router<S: DocumentStore + 'static>(
    plane: Arc<ControlPlane<S>>,
    token: AdminToken,
) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/scopes/{tenant}/{namespace}/resources", post(apply))
        .route("/v1/scopes/{tenant}/{namespace}/reconcile", post(reconcile))
        .route("/v1/scopes/{tenant}/{namespace}/snapshot", get(snapshot))
        .route("/v1/events", get(events))
        .layer(DefaultBodyLimit::max(ApiLimits::default().max_body_bytes))
        .with_state(ApiState { plane, token })
}

#[derive(Serialize)]
struct Status {
    status: &'static str,
    api_version: &'static str,
}
async fn status<S: DocumentStore + 'static>(
    State(state): State<ApiState<S>>,
    headers: HeaderMap,
) -> Response {
    match authorize(&state, &headers) {
        Ok(()) => Json(Status {
            status: "operational",
            api_version: "v1",
        })
        .into_response(),
        Err(error) => error_response(&error),
    }
}

#[derive(Debug, Deserialize)]
struct ApplyRequest {
    resource: DesiredResource,
    expected_revision: Option<u64>,
}
#[derive(Serialize)]
struct RevisionResponse {
    revision: u64,
}
async fn apply<S: DocumentStore + 'static>(
    State(state): State<ApiState<S>>,
    headers: HeaderMap,
    Path((tenant, namespace)): Path<(String, String)>,
    Json(request): Json<ApplyRequest>,
) -> Response {
    execute(&state, &headers, tenant, namespace, |scope| {
        let condition = request
            .expected_revision
            .map_or(WriteCondition::Create, WriteCondition::Match);
        state
            .plane
            .apply(&scope, request.resource, condition)
            .map(|revision| Json(RevisionResponse { revision }).into_response())
    })
}
async fn reconcile<S: DocumentStore + 'static>(
    State(state): State<ApiState<S>>,
    headers: HeaderMap,
    Path((tenant, namespace)): Path<(String, String)>,
) -> Response {
    execute(&state, &headers, tenant, namespace, |scope| {
        state
            .plane
            .reconcile(&scope)
            .map(|value| Json(value).into_response())
    })
}
async fn snapshot<S: DocumentStore + 'static>(
    State(state): State<ApiState<S>>,
    headers: HeaderMap,
    Path((tenant, namespace)): Path<(String, String)>,
) -> Response {
    execute(&state, &headers, tenant, namespace, |scope| {
        match state.plane.active(&scope)? {
            Some(value) => Ok(Json(value).into_response()),
            None => Ok(StatusCode::NOT_FOUND.into_response()),
        }
    })
}
async fn events<S: DocumentStore + 'static>(
    State(state): State<ApiState<S>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = authorize(&state, &headers) {
        return error_response(&error);
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from(
            "event: ready\ndata: {\"apiVersion\":\"v1\"}\n\n",
        ))
        .expect("static response")
}

fn execute<S: DocumentStore, F>(
    state: &ApiState<S>,
    headers: &HeaderMap,
    tenant: String,
    namespace: String,
    operation: F,
) -> Response
where
    F: FnOnce(StorageScope) -> Result<Response, AgentMeshError>,
{
    authorize(state, headers)
        .and_then(|()| StorageScope::new(tenant, namespace))
        .and_then(operation)
        .unwrap_or_else(|error| error_response(&error))
}
fn authorize<S>(state: &ApiState<S>, headers: &HeaderMap) -> Result<(), AgentMeshError> {
    if state.token.permits(headers) {
        Ok(())
    } else {
        Err(AgentMeshError::new(
            ErrorCode::Unauthenticated,
            "Administrative authentication is required.",
        ))
    }
}
fn error_response(error: &AgentMeshError) -> Response {
    let status = StatusCode::from_u16(error.code().http_status())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(error.public_response())).into_response()
}
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// API representation marker used by generated clients.
pub type SnapshotResponse = ConfigSnapshot;

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_storage::InMemoryStore;
    use axum::body::{Body, to_bytes};
    use tower::ServiceExt;

    #[tokio::test]
    async fn protects_and_reconciles_scoped_resources() {
        let app = router(
            Arc::new(ControlPlane::new(InMemoryStore::default())),
            AdminToken::new("0123456789abcdef").unwrap(),
        );
        let unauthorized = app
            .clone()
            .oneshot(
                http::Request::builder()
                    .uri("/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let body = serde_json::json!({"resource":{"kind":"route","name":"main","state":"active","spec":{"service":"search"}}});
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/scopes/acme/prod/resources")
            .header("authorization", "Bearer 0123456789abcdef")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/scopes/acme/prod/reconcile")
            .header("authorization", "Bearer 0123456789abcdef")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(
            serde_json::from_slice::<ConfigSnapshot>(&bytes)
                .unwrap()
                .verify()
        );
    }
}
