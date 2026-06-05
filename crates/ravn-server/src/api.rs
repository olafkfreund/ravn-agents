//! HTTP API: the router, the system endpoints, and the OpenAPI document.
//!
//! This is the base everything else hangs off (#23). Ingestion, registry, and
//! auth routes are added by their own issues (#24, #25, #26).

use std::collections::BTreeMap;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;

use crate::db;
use crate::db::{Agent, CategoryDimension, StoredEvent, Topology};
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

/// Query parameters for the topology view.
#[derive(Debug, Deserialize, IntoParams)]
pub struct TopologyParams {
    /// Label key to group agents by (e.g. `env`). Omit for a single group.
    pub group_by: Option<String>,
}

/// The fleet shaped for the topology diagram.
#[utoipa::path(get, path = "/api/topology", tag = "agents",
    params(TopologyParams),
    responses((status = 200, description = "Grouped fleet", body = Topology)))]
async fn topology(
    State(state): State<AppState>,
    Query(params): Query<TopologyParams>,
) -> Result<Json<Topology>, ApiError> {
    Ok(Json(db::topology(&state.pool, params.group_by.as_deref()).await?))
}

/// Live event stream (#29). Upgrades to a WebSocket that emits each newly
/// ingested event as JSON.
async fn ws_events(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| stream_events(socket, state))
}

async fn stream_events(mut socket: WebSocket, state: AppState) {
    let mut rx = state.events_tx.subscribe();
    loop {
        tokio::select! {
            received = rx.recv() => match received {
                Ok(event) => {
                    let json = match serde_json::to_string(&event) {
                        Ok(j) => j,
                        Err(_) => continue,
                    };
                    if socket.send(WsMessage::Text(json.into())).await.is_err() {
                        break; // client gone
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(_)) => {}      // ignore client messages (incl. pings handled by axum)
                _ => break,            // closed or errored
            },
        }
    }
}

/// The OpenAPI specification for the control plane.
#[derive(OpenApi)]
#[openapi(
    paths(health, ready, list_events, list_agents, get_agent, put_labels, delete_agent, list_categories, topology),
    components(schemas(Health, Readiness, StoredEvent, Agent, CategoryDimension, crate::db::CategoryValue,
        Topology, crate::db::TopologyGroup, crate::db::TopologyNode)),
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

/// Prometheus metrics in text exposition format (#40). Public.
async fn metrics(State(state): State<AppState>) -> Response {
    if let Ok(agents) = db::list_agents(&state.pool).await {
        let (mut online, mut stale, mut offline) = (0i64, 0i64, 0i64);
        for a in &agents {
            match a.status.as_str() {
                "online" => online += 1,
                "stale" => stale += 1,
                _ => offline += 1,
            }
        }
        state.metrics.set_agent_counts(online, stale, offline);
    }
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        state.metrics.encode(),
    )
        .into_response()
}

/// Agent enrollment (#19): exchange a bootstrap token + CSR for a signed
/// client certificate. Authenticated by the bootstrap token itself (so it is
/// exempt from bearer auth), compared in constant time. Idempotent.
async fn enroll(
    State(state): State<AppState>,
    Json(req): Json<ravn_core::EnrollRequest>,
) -> Response {
    let (Some(ca), Some(expected)) = (state.ca.as_ref(), state.enroll_token.as_deref()) else {
        return (StatusCode::NOT_FOUND, "enrollment is not enabled").into_response();
    };

    if !constant_time_eq(req.bootstrap_token.as_bytes(), expected.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "invalid bootstrap token").into_response();
    }

    let (certificate_pem, not_after) = match ca.sign(&req.csr_pem, req.agent_id.0) {
        Ok(v) => v,
        Err(error) => {
            tracing::warn!(%error, agent_id = %req.agent_id.0, "enrollment CSR signing failed");
            return (StatusCode::BAD_REQUEST, "could not sign CSR").into_response();
        }
    };

    let not_after = chrono::DateTime::from_timestamp(not_after.unix_timestamp(), 0)
        .unwrap_or_else(chrono::Utc::now);

    if let Err(error) =
        db::record_enrollment(&state.pool, req.agent_id.0, &req.host, not_after).await
    {
        tracing::error!(%error, "recording enrollment failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "could not record enrollment").into_response();
    }

    tracing::info!(agent_id = %req.agent_id.0, host = %req.host, %not_after, "agent enrolled");
    Json(ravn_core::EnrollResponse {
        agent_id: req.agent_id,
        certificate_pem,
        ca_certificate_pem: ca.ca_cert_pem().to_string(),
        command_signing_pubkey: state.command_signer.pubkey_b64().to_string(),
        not_after,
    })
    .into_response()
}

/// Pull pending remediation commands for an agent (#114). The agent long-polls
/// this over its outbound connection, verifies each signed [`CommandEnvelope`]
/// against the pinned control-plane key, and executes via the actuator.
///
/// Auth: behind the API's bearer middleware in P1. Per-agent authentication of
/// the puller (so an agent can only read *its own* commands) is the remaining
/// transport-auth work (#26); signing already prevents any forged command from
/// being executed regardless of who pulls.
async fn get_commands(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> Json<Vec<ravn_core::CommandEnvelope>> {
    Json(state.command_queue.take_for(agent_id))
}

/// Report the outcome of a command (#114): the agent POSTs an [`ActionResult`]
/// after the actuator runs (or rejects) it.
async fn post_command_result(
    State(state): State<AppState>,
    Path((_agent_id, _command_id)): Path<(Uuid, Uuid)>,
    Json(result): Json<ravn_core::ActionResult>,
) -> StatusCode {
    tracing::info!(command_id = %result.command_id, status = ?result.status, "command result reported");
    state.command_queue.record_result(result);
    StatusCode::ACCEPTED
}

/// Authenticated HTTP ingest (#57): in-cluster agents POST a [`Message`] with
/// their projected ServiceAccount token as a bearer credential. The token is
/// validated against the cluster's OIDC JWKS (#57) and/or via Kubernetes
/// TokenReview (#102). Self-authenticating, so it is exempt from the API's
/// admin/viewer bearer auth.
async fn ingest(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(message): Json<ravn_core::Message>,
) -> Response {
    if state.ingest_auth.is_none() && state.ingest_token_review.is_none() {
        return (StatusCode::NOT_FOUND, "authenticated ingest is not enabled").into_response();
    }

    let token = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };

    // Try local JWKS validation first (no network); fall back to TokenReview.
    let mut identity = state.ingest_auth.as_ref().and_then(|a| a.validate(token).ok());
    if identity.is_none() {
        if let Some(tr) = state.ingest_token_review.as_ref() {
            match tr.validate(token).await {
                Ok(claims) => identity = Some(claims),
                Err(error) => tracing::debug!(%error, "TokenReview rejected token"),
            }
        }
    }

    match identity {
        Some(claims) => {
            tracing::debug!(sub = %claims.sub, event_id = %message.event.id, "authenticated ingest");
            crate::ingest::persist_message(&state, &message).await;
            (StatusCode::ACCEPTED, "accepted").into_response()
        }
        None => {
            tracing::warn!("rejected ingest: invalid ServiceAccount token");
            (StatusCode::UNAUTHORIZED, "invalid token").into_response()
        }
    }
}

/// Look up a key in a URL query string. Values are returned as-is — JWT bearer
/// tokens contain only URL-safe characters, so no decoding is needed (#101).
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// Constant-time byte comparison, to keep token checks free of timing leaks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Endpoints reachable without auth even when tokens are configured.
fn is_public(path: &str) -> bool {
    matches!(path, "/ready" | "/metrics" | "/enroll" | "/ingest" | "/auth/config")
}

/// The caller's resolved role (#26), for role-aware portal UI. Behind auth, so
/// a missing role extension means auth is disabled → full (admin) access.
async fn me(role: Option<axum::Extension<Role>>) -> Response {
    let role = role.map(|axum::Extension(r)| r).unwrap_or(Role::Admin);
    let role = match role {
        Role::Admin => "admin",
        Role::Viewer => "viewer",
    };
    Json(serde_json::json!({ "role": role })).into_response()
}

/// Public auth configuration for the SPA (#26): whether user OIDC is enabled
/// and, if so, the issuer / client id / scopes it needs to start the
/// auth-code+PKCE flow. No secrets.
async fn auth_config(State(state): State<AppState>) -> Response {
    match &state.oidc_public {
        Some(o) => Json(serde_json::json!({
            "enabled": true,
            "issuer": o.issuer,
            "clientId": o.client_id,
            "scopes": o.scopes,
        }))
        .into_response(),
        None => Json(serde_json::json!({ "enabled": false })).into_response(),
    }
}

use crate::auth::Role;

/// Resolve a static bearer token to a role (dev/bootstrap path).
fn role_for(token: &str, admin: Option<&str>, viewer: Option<&str>) -> Option<Role> {
    if admin == Some(token) {
        Some(Role::Admin)
    } else if viewer == Some(token) {
        Some(Role::Viewer)
    } else {
        None
    }
}

/// Whether `role` may perform a request with this method (safe methods need a
/// viewer; mutating methods need admin).
fn authorized(method: &axum::http::Method, role: Role) -> bool {
    method.is_safe() || role == Role::Admin
}

/// Bearer-token auth + RBAC. No-op when no auth is configured. Accepts a static
/// admin/viewer token (dev/bootstrap) or a portal user OIDC token (#26), whose
/// role is derived from the user's groups.
async fn auth_mw(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let auth_enabled =
        state.admin_token.is_some() || state.viewer_token.is_some() || state.user_auth.is_some();
    if !auth_enabled {
        return next.run(req).await; // auth disabled
    }
    if is_public(req.uri().path()) {
        return next.run(req).await; // probes + metrics + auth-config are public
    }

    // Prefer the Authorization header; fall back to the `access_token` query
    // param. Browsers cannot set headers on a WebSocket upgrade, so the live
    // feed (/ws/events) presents its bearer in the query (#101).
    let header_token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_string);
    let token = header_token.or_else(|| query_param(req.uri().query(), "access_token"));
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let token = token.as_str();

    // Static token first (cheap, no crypto); then a user OIDC token.
    let mut role = role_for(token, state.admin_token.as_deref(), state.viewer_token.as_deref());
    if role.is_none() {
        if let Some(user_auth) = state.user_auth.as_ref() {
            match user_auth.authorize(token) {
                Ok(Some(r)) => role = Some(r),
                Ok(None) => {
                    return (StatusCode::FORBIDDEN, "authenticated but not in an allowed group")
                        .into_response()
                }
                Err(error) => tracing::debug!(%error, "user OIDC token rejected"),
            }
        }
    }

    let Some(role) = role else {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    };

    if authorized(req.method(), role) {
        // Surface the resolved role to handlers (e.g. /api/me).
        let mut req = req;
        req.extensions_mut().insert(role);
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "admin role required").into_response()
    }
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
        .route("/api/topology", get(topology))
        .route("/ws/events", get(ws_events))
        .route("/metrics", get(metrics))
        .route("/enroll", axum::routing::post(enroll))
        .route("/ingest", axum::routing::post(ingest))
        .route("/agents/{id}/commands", get(get_commands))
        .route("/agents/{id}/commands/{cmd}/result", axum::routing::post(post_command_result))
        .route("/auth/config", get(auth_config))
        .route("/api/me", get(me))
        .layer(middleware::from_fn_with_state(state.clone(), auth_mw))
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
    #[test]
    fn query_param_extracts_access_token() {
        assert_eq!(query_param(Some("a=1&access_token=abc.def.ghi&b=2"), "access_token").as_deref(), Some("abc.def.ghi"));
        assert_eq!(query_param(Some("access_token=xyz"), "access_token").as_deref(), Some("xyz"));
        assert_eq!(query_param(Some("foo=bar"), "access_token"), None);
        assert_eq!(query_param(None, "access_token"), None);
    }

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

    #[test]
    fn role_resolution() {
        assert_eq!(role_for("a", Some("a"), Some("v")), Some(Role::Admin));
        assert_eq!(role_for("v", Some("a"), Some("v")), Some(Role::Viewer));
        assert_eq!(role_for("x", Some("a"), Some("v")), None);
        assert_eq!(role_for("a", None, Some("v")), None);
    }

    #[test]
    fn rbac_by_method() {
        use axum::http::Method;
        // Viewer: read-only.
        assert!(authorized(&Method::GET, Role::Viewer));
        assert!(!authorized(&Method::PUT, Role::Viewer));
        assert!(!authorized(&Method::DELETE, Role::Viewer));
        // Admin: everything.
        assert!(authorized(&Method::GET, Role::Admin));
        assert!(authorized(&Method::PUT, Role::Admin));
        assert!(authorized(&Method::DELETE, Role::Admin));
    }
}
