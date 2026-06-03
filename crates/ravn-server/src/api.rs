//! HTTP API: the router, the system endpoints, and the OpenAPI document.
//!
//! This is the base everything else hangs off (#23). Ingestion, registry, and
//! auth routes are added by their own issues (#24, #25, #26).

use std::collections::BTreeMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;

use crate::db;
use crate::db::{Agent, CategoryDimension, StoredEvent};
use crate::state::AppState;

/// Maximum number of events returned in one page.
const MAX_LIMIT: i64 = 500;
const DEFAULT_LIMIT: i64 = 50;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Liveness response.
#[derive(Debug, Serialize, ToSchema)]
pub struct Health {
    /// Always `"ok"` when the service is live.
    pub status: String,
    /// Build version of the control plane.
    pub version: String,
}

/// Readiness response.
#[derive(Debug, Serialize, ToSchema)]
pub struct Readiness {
    /// Whether the service is ready to accept traffic.
    pub ready: bool,
    /// Per-dependency readiness (e.g. database, NATS), populated as those
    /// dependencies are wired in (#24).
    pub checks: BTreeMap<String, bool>,
}

/// Liveness probe — the process is up and serving.
#[utoipa::path(
    get,
    path = "/health",
    tag = "system",
    responses((status = 200, description = "Service is live", body = Health))
)]
async fn health() -> Json<Health> {
    Json(Health { status: "ok".to_string(), version: VERSION.to_string() })
}

/// Readiness probe — the process can serve real traffic.
#[utoipa::path(
    get,
    path = "/ready",
    tag = "system",
    responses((status = 200, description = "Service is ready", body = Readiness))
)]
async fn ready(State(state): State<AppState>) -> Json<Readiness> {
    let database = db::ping(&state.pool).await;
    let nats = matches!(
        state.nats.connection_state(),
        async_nats::connection::State::Connected
    );

    let mut checks = BTreeMap::new();
    checks.insert("database".to_string(), database);
    checks.insert("nats".to_string(), nats);

    Json(Readiness { ready: database && nats, checks })
}

/// Query parameters for listing events.
#[derive(Debug, Deserialize, IntoParams)]
pub struct ListEventsParams {
    /// Maximum number of events to return (1–500, default 50).
    pub limit: Option<i64>,
}

/// An error response.
struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self.0, "request failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        ApiError(err.into())
    }
}

/// List recent events, newest first.
#[utoipa::path(
    get,
    path = "/api/events",
    tag = "events",
    params(ListEventsParams),
    responses((status = 200, description = "Recent events", body = [StoredEvent]))
)]
async fn list_events(
    State(state): State<AppState>,
    Query(params): Query<ListEventsParams>,
) -> Result<Json<Vec<StoredEvent>>, ApiError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let events = db::recent_events(&state.pool, limit).await?;
    Ok(Json(events))
}

/// List all registered agents with status and labels.
#[utoipa::path(get, path = "/api/agents", tag = "agents",
    responses((status = 200, description = "Registered agents", body = [Agent])))]
async fn list_agents(State(state): State<AppState>) -> Result<Json<Vec<Agent>>, ApiError> {
    Ok(Json(db::list_agents(&state.pool).await?))
}

/// Fetch a single agent.
#[utoipa::path(get, path = "/api/agents/{id}", tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    responses((status = 200, body = Agent), (status = 404, description = "Unknown agent")))]
async fn get_agent(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match db::get_agent(&state.pool, id).await {
        Ok(Some(agent)) => Json(agent).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "agent not found").into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

/// Replace an agent's labels.
#[utoipa::path(put, path = "/api/agents/{id}/labels", tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    request_body = std::collections::BTreeMap<String, String>,
    responses((status = 200, body = Agent), (status = 404, description = "Unknown agent")))]
async fn put_labels(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(labels): Json<BTreeMap<String, String>>,
) -> Response {
    match db::replace_labels(&state.pool, id, &labels).await {
        Ok(false) => (StatusCode::NOT_FOUND, "agent not found").into_response(),
        Ok(true) => match db::get_agent(&state.pool, id).await {
            Ok(Some(agent)) => Json(agent).into_response(),
            Ok(None) => (StatusCode::NOT_FOUND, "agent not found").into_response(),
            Err(error) => ApiError(error).into_response(),
        },
        Err(error) => ApiError(error).into_response(),
    }
}

/// Remove an agent and its labels.
#[utoipa::path(delete, path = "/api/agents/{id}", tag = "agents",
    params(("id" = Uuid, Path, description = "Agent id")),
    responses((status = 204, description = "Deleted"), (status = 404, description = "Unknown agent")))]
async fn delete_agent(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match db::delete_agent(&state.pool, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "agent not found").into_response(),
        Err(error) => ApiError(error).into_response(),
    }
}

/// List grouping dimensions (label keys, values, and agent counts).
#[utoipa::path(get, path = "/api/categories", tag = "agents",
    responses((status = 200, description = "Grouping dimensions", body = [CategoryDimension])))]
async fn list_categories(State(state): State<AppState>) -> Result<Json<Vec<CategoryDimension>>, ApiError> {
    Ok(Json(db::list_categories(&state.pool).await?))
}

/// The OpenAPI specification for the control plane.
#[derive(OpenApi)]
#[openapi(
    paths(health, ready, list_events, list_agents, get_agent, put_labels, delete_agent, list_categories),
    components(schemas(Health, Readiness, StoredEvent, Agent, CategoryDimension, crate::db::CategoryValue)),
    info(
        title = "Ravn control plane",
        description = "Ingests agent events, persists them, and serves the portal."
    ),
    tags(
        (name = "system", description = "Liveness and readiness probes"),
        (name = "events", description = "Persisted detection events"),
        (name = "agents", description = "Agent registry and categories")
    )
)]
pub struct ApiDoc;

/// Serve the OpenAPI document as JSON.
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Stateless system routes (liveness + the OpenAPI document).
fn system_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi_json))
}

/// Build the full application router for the given state.
pub fn router(state: AppState) -> Router {
    let stateful = Router::new()
        .route("/ready", get(ready))
        .route("/api/events", get(list_events))
        .route("/api/agents", get(list_agents))
        .route("/api/agents/{id}", get(get_agent).delete(delete_agent))
        .route("/api/agents/{id}/labels", axum::routing::put(put_labels))
        .route("/api/categories", get(list_categories))
        .with_state(state);

    system_router()
        .merge(stateful)
        // Permissive CORS for the M0 dev portal (tighten before production).
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    // Exercises only the stateless system routes — no DB/NATS required, so
    // these run in the hermetic Nix build. `/ready` is covered by an ignored
    // live integration test (see tests below) that needs real services.
    async fn get_json(path: &str) -> (StatusCode, serde_json::Value) {
        let resp = system_router()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn health_reports_ok() {
        let (status, body) = get_json("/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["version"], VERSION);
    }

    #[tokio::test]
    async fn openapi_document_is_served() {
        let (status, body) = get_json("/openapi.json").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["openapi"].as_str().unwrap().starts_with("3."));
        assert!(body["paths"]["/health"].is_object());
        assert!(body["paths"]["/ready"].is_object());
    }
}
