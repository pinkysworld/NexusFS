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
        Some(t) if constant_time_eq(t.as_bytes(), expected.as_bytes()) => Ok(()),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Compare two secrets without leaking their common prefix through timing.
///
/// `==` short-circuits at the first mismatch. The console is expected on loopback, where
/// this is hard to exploit, but nothing enforces that and the fix is three lines.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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
        .route("/api/storage/gc", get(storage_gc))
        .route("/api/security", get(security))
        .route("/api/identity", get(identity))
        .route("/api/peers/enrolled", get(enrolled_peers))
        .route("/api/fs/cat", get(fs_cat))
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
    /// The on-disk format the store is stamped with. `None` on a store that has not
    /// been opened by a versioning-aware build yet, which should not happen in practice
    /// because opening stamps it.
    format_version: Option<u32>,
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
            format_version: st.core.format_version()?,
        })
    };
    Ok(Json(
        build().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    ))
}

#[derive(Serialize)]
struct IdentityResp {
    device_id: String,
    /// Hex ed25519 public key, or `None` on a build that did not supply one.
    pubkey: Option<String>,
    format_version: Option<u32>,
    /// The format this build expects, so a mismatch is visible without reading logs.
    expects_format: u32,
    build_version: &'static str,
}

/// What another node needs in order to enrol this one.
///
/// Both fields are public by construction — a device id is an opaque identifier and the
/// key is the public half — so this carries nothing the peer would not learn on its
/// first handshake anyway.
async fn identity(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<IdentityResp>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;
    Ok(Json(IdentityResp {
        device_id: format!("{:x}", st.core.device_id.0),
        pubkey: st.node_pubkey.map(hex::encode),
        format_version: st.core.format_version().map_err(server_error)?,
        expects_format: nexusfs_core::CURRENT_FORMAT_VERSION,
        build_version: env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Serialize)]
struct EnrolledPeerResp {
    device_id: String,
    pubkey: String,
}

/// The keys this node has pinned.
///
/// Distinct from `/api/peers`, which reports the *sync* targets and how they are doing.
/// A device can be trusted without being a configured peer (it may connect inbound), and
/// a configured peer may not be trusted yet — so conflating the two lists would hide
/// exactly the mismatch an operator is looking for.
async fn enrolled_peers(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<Vec<EnrolledPeerResp>>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;
    let peers = st.core.enrolled_peers().map_err(server_error)?;
    Ok(Json(
        peers
            .into_iter()
            .map(|p| EnrolledPeerResp {
                device_id: format!("{:x}", p.device_id.0),
                pubkey: hex::encode(p.pubkey),
            })
            .collect(),
    ))
}

/// Largest file the console will render inline.
///
/// The console is for inspecting state, not for transferring data; anything larger is
/// better fetched with `nexusfs cat`. Capping also stops one click on a multi-gigabyte
/// file from pinning that much memory in the daemon.
const MAX_INLINE_BYTES: usize = 256 * 1024;

#[derive(Serialize)]
struct CatResp {
    path: String,
    size: u64,
    /// "text" when the prefix decoded as UTF-8, "binary" otherwise.
    kind: &'static str,
    /// True when `content` holds only the first [`MAX_INLINE_BYTES`].
    truncated: bool,
    /// Present for text files only. Binary content is described, never dumped.
    content: Option<String>,
    /// Hex of the first bytes, for binary files — enough to recognise a magic number.
    preview_hex: Option<String>,
}

async fn fs_cat(
    State(st): State<AdminState>,
    headers: HeaderMap,
    Query(q): Query<LsQuery>,
) -> Result<Json<CatResp>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;

    let bytes = st.core.read_file_path(&q.path).map_err(server_error)?;
    let size = bytes.len() as u64;
    let truncated = bytes.len() > MAX_INLINE_BYTES;
    let head = &bytes[..bytes.len().min(MAX_INLINE_BYTES)];

    // Decoding the prefix rather than the whole file keeps the check cheap, and a
    // truncated multi-byte character at the cut is treated as binary rather than
    // rendered as a replacement glyph.
    match std::str::from_utf8(head) {
        Ok(text) => Ok(Json(CatResp {
            path: q.path,
            size,
            kind: "text",
            truncated,
            content: Some(text.to_string()),
            preview_hex: None,
        })),
        Err(_) => Ok(Json(CatResp {
            path: q.path,
            size,
            kind: "binary",
            truncated,
            content: None,
            preview_hex: Some(hex::encode(&head[..head.len().min(64)])),
        })),
    }
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

#[derive(Serialize)]
struct ClockEntry {
    device_id: String,
    /// Highest counter applied with no gap below it.
    through: u64,
}

#[derive(Serialize)]
struct ClockResp {
    entries: Vec<ClockEntry>,
}

/// Per-device replication progress.
///
/// Device ids are hex strings here, not numbers. A `DeviceId` is a `u128`, and a JSON
/// number that large loses precision the moment a JavaScript client parses it — the
/// console would render a device id that is close to the real one and wrong, which is
/// worse than not showing it at all.
async fn oplog_summary(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    require_token(&headers, &st.token)?;
    let sum = st
        .core
        .clock_summary()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(ClockResp {
        entries: sum
            .entries
            .into_iter()
            .map(|(device, through)| ClockEntry {
                device_id: format!("{:x}", device.0),
                through,
            })
            .collect(),
    }))
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
        Unlink { parent, name, .. } => format!("unlink {name} from {parent:x}"),
        SetAttr { inode, .. } => format!("setattr {inode:x}"),
    }
}

#[derive(Serialize)]
struct PeersResp {
    /// False when replication is not compiled in or failed to start.
    enabled: bool,
    peers: Vec<crate::PeerView>,
}

/// Survey unreachable storage.
///
/// Read-only on purpose. The daemon is writing while this serves, so a blob created
/// between the mark and a sweep would look like garbage — collection belongs in
/// `nexusfs gc`, which holds the store exclusively.
async fn storage_gc(
    State(st): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<nexusfs_core::GcReport>, (StatusCode, String)> {
    require_token(&headers, &st.token).map_err(|c| (c, "unauthorized".into()))?;
    st.core
        .collect_garbage(true)
        .map(Json)
        .map_err(server_error)
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
