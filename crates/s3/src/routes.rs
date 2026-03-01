use axum::{routing::get, Router};

use crate::S3State;

/// Skeleton S3-like router.
///
/// TODO: implement PUT/GET/DELETE/LIST mapping to directories and files.
pub fn router(_st: S3State) -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}
