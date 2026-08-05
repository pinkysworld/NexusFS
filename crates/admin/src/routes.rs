use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use nexusfs_core::EntryType;

use crate::assets;
use crate::{AdminState, EnergyView};

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

/// Surface the real error to the operator instead of a bare 500 — this console
/// exists to make local state debuggable.
fn server_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
}

pub fn router(state: AdminState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/api/status", get(status))
        .route("/api/fs/head", get(head))
        .route("/api/oplog/summary", get(oplog_summary))
        .route("/api/storage/stats", get(storage_stats))
        .route("/api/fs/ls", get(fs_ls))
        .route("/api/oplog/recent", get(oplog_recent))
        .route("/api/peers", get(peers))
        .route("/api/energy", get(energy))
        .route("/api/security", get(security))
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
    state_root: Option<String>,
    device_id: String,
    ops: usize,
    applied: usize,
    pending: usize,
    now_ms: u64,
}

async fn status(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<Status>, StatusCode> {
    require_token(&headers, &st.token)?;
    let build = || -> anyhow::Result<Status> {
        Ok(Status {
            head: st.core.get_head()?.map(hex::encode),
            state_root: st.core.get_state_root()?.map(hex::encode),
            device_id: format!("{:x}", st.core.device_id.0),
            ops: st.core.op_count()?,
            applied: st.core.applied_count()?,
            pending: st.core.pending_count()?,
            now_ms: nexusfs_core::now_ms(),
        })
    };
    Ok(Json(
        build().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
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

#[derive(Serialize)]
struct SecurityResp {
    encryption_at_rest: bool,
    proof_policy: String,
    ops_total: usize,
    ops_with_proof: usize,
    ops_without_proof: usize,
    malformed_proofs: usize,
    signature_failures: usize,
    unreadable_files: Vec<String>,
    healthy: bool,
}

/// Audit the repository and report what the check found.
///
/// Reads every file, so it is deliberately not on the status page's refresh path.
async fn security(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<SecurityResp>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    let report = st.core.verify_repository().map_err(server_error)?;
    Ok(Json(SecurityResp {
        encryption_at_rest: st.core.encryption_enabled(),
        proof_policy: format!("{:?}", st.core.proofs).to_lowercase(),
        ops_total: report.operations,
        ops_with_proof: report.with_proof,
        ops_without_proof: report.without_proof,
        malformed_proofs: report.malformed,
        signature_failures: report.signature_failures,
        healthy: report.ok(),
        unreadable_files: report.unreadable_files,
    }))
}

#[derive(Serialize)]
struct StorageStats {
    blob_count: usize,
    blob_bytes: u64,
    state_entries: usize,
    op_count: usize,
    applied_count: usize,
    pending_count: usize,
}

async fn storage_stats(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<StorageStats>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    let build = || -> anyhow::Result<StorageStats> {
        let (blob_count, blob_bytes) = st.core.blob_stats()?;
        Ok(StorageStats {
            blob_count,
            blob_bytes,
            state_entries: st.core.state_entry_count()?,
            op_count: st.core.op_count()?,
            applied_count: st.core.applied_count()?,
            pending_count: st.core.pending_count()?,
        })
    };
    Ok(Json(build().map_err(server_error)?))
}

#[derive(Deserialize)]
struct LsQuery {
    #[serde(default = "root_path")]
    path: String,
}

fn root_path() -> String {
    "/".to_string()
}

#[derive(Serialize)]
struct LsEntry {
    name: String,
    inode: String,
    kind: &'static str,
    size: u64,
}

#[derive(Serialize)]
struct LsResp {
    path: String,
    entries: Vec<LsEntry>,
}

async fn fs_ls(
    State(st): State<AdminState>,
    headers: HeaderMap,
    Query(q): Query<LsQuery>,
) -> Result<Json<LsResp>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    let build = || -> anyhow::Result<LsResp> {
        let mut entries = Vec::new();
        for entry in st.core.read_dir_path(&q.path)? {
            let size = match entry.entry_type {
                EntryType::File => st
                    .core
                    .load_inode(entry.inode_id)?
                    .map(|r| r.content.value.size)
                    .unwrap_or(0),
                EntryType::Dir => 0,
            };
            entries.push(LsEntry {
                name: entry.name,
                inode: format!("{:x}", entry.inode_id),
                kind: match entry.entry_type {
                    EntryType::Dir => "dir",
                    EntryType::File => "file",
                },
                size,
            });
        }
        Ok(LsResp {
            path: q.path.clone(),
            entries,
        })
    };

    Ok(Json(build().map_err(server_error)?))
}

#[derive(Deserialize)]
struct RecentQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

#[derive(Serialize)]
struct OpSummary {
    device_id: String,
    counter: u64,
    time_unix_ms: u64,
    kind: String,
    applied: bool,
}

async fn oplog_recent(
    State(st): State<AdminState>,
    headers: HeaderMap,
    Query(q): Query<RecentQuery>,
) -> Result<Json<Vec<OpSummary>>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    let build = || -> anyhow::Result<Vec<OpSummary>> {
        let mut out = Vec::new();
        for op in st.core.recent_ops(q.limit.min(500))? {
            out.push(OpSummary {
                device_id: format!("{:x}", op.id.device_id.0),
                counter: op.id.counter,
                time_unix_ms: op.time_unix_ms,
                kind: describe_op(&op.kind),
                applied: st.core.is_op_applied(op.id)?,
            });
        }
        Ok(out)
    };

    Ok(Json(build().map_err(server_error)?))
}

fn describe_op(kind: &nexusfs_proto::FsOpKind) -> String {
    use nexusfs_proto::FsOpKind::*;
    match kind {
        Mkdir { parent, name, .. } => format!("mkdir {name} in {parent:x}"),
        CreateFile { parent, name, .. } => format!("create {name} in {parent:x}"),
        Write {
            inode,
            chunks,
            new_size,
            ..
        } => format!(
            "write {inode:x} ({new_size} bytes, {} chunks)",
            chunks.len()
        ),
        Rename {
            old_name, new_name, ..
        } => format!("rename {old_name} -> {new_name}"),
        Unlink { parent, name } => format!("unlink {name} from {parent:x}"),
        SetAttr { inode, .. } => format!("setattr {inode:x}"),
    }
}

#[derive(Serialize)]
struct PeersResp {
    /// False when replication is not compiled in or failed to start.
    enabled: bool,
    peers: Vec<crate::PeerView>,
}

/// The current power reading and the replication budget it produced.
///
/// `enabled: false` means the daemon is not sampling — not that the device is
/// unconstrained. The console shows those differently on purpose.
async fn energy(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<EnergyView>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    Ok(Json(match &st.energy {
        Some(source) => source.energy(),
        None => EnergyView {
            enabled: false,
            power: "unknown".into(),
            link: "unknown".into(),
            sync: true,
            content: true,
            max_content_bytes: None,
            interval_scale: 1.0,
            reason: "energy-aware scheduling is not running".into(),
            ..Default::default()
        },
    }))
}

async fn peers(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<PeersResp>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    Ok(Json(match &st.peers {
        Some(source) => PeersResp {
            enabled: true,
            peers: source.peers(),
        },
        None => PeersResp {
            enabled: false,
            peers: vec![],
        },
    }))
}
