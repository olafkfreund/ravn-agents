//! HTTP API: the router, the system endpoints, and the OpenAPI document.
//!
//! This is the base everything else hangs off (#23). Ingestion, registry, and
//! auth routes are added by their own issues (#24, #25, #26).

use std::collections::BTreeMap;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use tower_http::trace::TraceLayer;
use utoipa::{OpenApi, ToSchema};

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
async fn ready() -> Json<Readiness> {
    // No external dependencies are wired yet (#24), so the service is ready by
    // definition. As dependencies land they each add a `checks` entry and can
    // flip `ready` to false.
    Json(Readiness { ready: true, checks: BTreeMap::new() })
}

/// The OpenAPI specification for the control plane.
#[derive(OpenApi)]
#[openapi(
    paths(health, ready),
    components(schemas(Health, Readiness)),
    info(
        title = "Ravn control plane",
        description = "Ingests agent events, persists them, and serves the portal."
    ),
    tags((name = "system", description = "Liveness and readiness probes"))
)]
pub struct ApiDoc;

/// Serve the OpenAPI document as JSON.
async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Build the application router.
pub fn app() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/openapi.json", get(openapi_json))
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt; // for `oneshot`

    async fn get_json(path: &str) -> (StatusCode, serde_json::Value) {
        let resp = app()
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
    async fn ready_reports_ready() {
        let (status, body) = get_json("/ready").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ready"], true);
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
