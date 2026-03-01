use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;

use crate::assets;
use crate::AdminState;

/// Very small auth middleware: check `x-nexusfs-token` header.
/// In production, add mTLS or OAuth flows.
fn require_token(headers: &HeaderMap, expected: &str) -> Result<(), StatusCode> {
    if expected.is_empty() {
        // If token is empty, treat as dev mode (allow). The daemon should generate one if missing.
        return Ok(());
    }
    match headers.get("x-nexusfs-token").and_then(|v| v.to_str().ok()) {
        Some(t) if t == expected => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

pub fn router(state: AdminState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/api/status", get(status))
        .route("/api/fs/head", get(head))
        .route("/api/oplog/summary", get(oplog_summary))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(std::str::from_utf8(assets::INDEX_HTML).unwrap_or("invalid utf8"))
}

async fn app_js() -> Response {
    let body = assets::APP_JS;
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        body,
    )
        .into_response()
}

#[derive(Serialize)]
struct Status {
    head: Option<String>,
    device_id: String,
    now_ms: u64,
}

async fn status(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<Status>, StatusCode> {
    require_token(&headers, &st.token)?;
    let head = st
        .core
        .get_head()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let head_hex = head.map(hex::encode);
    Ok(Json(Status {
        head: head_hex,
        device_id: format!("{:x}", st.core.device_id.0),
        now_ms: nexusfs_core::now_ms(),
    }))
}

#[derive(Serialize)]
struct HeadResp {
    head: Option<String>,
}

async fn head(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<HeadResp>, StatusCode> {
    require_token(&headers, &st.token)?;
    let head = st
        .core
        .get_head()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(HeadResp {
        head: head.map(hex::encode),
    }))
}

async fn oplog_summary(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    require_token(&headers, &st.token)?;
    let sum = st
        .core
        .clock_summary()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(sum))
}
