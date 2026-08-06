//! S3-compatible subset over the NexusFS namespace.
//!
//! Mapping: a bucket is a top-level directory, and an object key is the path beneath
//! it. `PUT /photos/2024/june/a.jpg` writes `/photos/2024/june/a.jpg`, creating the
//! intermediate directories — which is how S3's flat keyspace and a real tree coexist
//! without a separate index.
//!
//! Every mutation goes through `CoreState`'s high-level operations, so an S3 write
//! produces exactly the same signed operations as the equivalent CLI command. The
//! facade cannot bypass oplog semantics because it has no way to reach past them.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Deserialize;

use nexusfs_core::{now_ms, CoreState, EntryType};

use crate::xml::{self, ObjectRow};
use crate::S3State;

/// S3 caps a single listing page at 1000 keys.
const MAX_KEYS_LIMIT: usize = 1000;

/// Largest object this facade accepts in one request.
///
/// A PUT is buffered whole before it is chunked, so this is a real memory bound rather
/// than a policy. It has to be stated explicitly: axum defaults `Bytes` to 2 MB, which
/// silently made every upload above that fail with a framework error that was not even
/// S3-shaped. Larger objects need multipart upload, which this subset does not
/// implement yet.
const MAX_OBJECT_BYTES: usize = 64 * 1024 * 1024;

pub fn router(state: S3State) -> Router {
    Router::new()
        .route("/", get(list_buckets))
        .route("/health", get(|| async { "ok" }))
        .route("/:bucket", get(list_objects).put(create_bucket))
        .route(
            "/:bucket/*key",
            get(get_object)
                .head(head_object)
                .put(put_object)
                .delete(delete_object),
        )
        .layer(DefaultBodyLimit::max(MAX_OBJECT_BYTES))
        .with_state(state)
}

// --- errors ----------------------------------------------------------------

/// An S3 error: XML body plus the status code clients switch on.
struct S3Error(StatusCode, &'static str, String, String);

impl IntoResponse for S3Error {
    fn into_response(self) -> Response {
        let S3Error(status, code, message, resource) = self;
        (
            status,
            [(header::CONTENT_TYPE, "application/xml")],
            xml::error(code, &message, &resource),
        )
            .into_response()
    }
}

fn no_such_key(resource: &str) -> S3Error {
    S3Error(
        StatusCode::NOT_FOUND,
        "NoSuchKey",
        "The specified key does not exist.".into(),
        resource.into(),
    )
}

fn no_such_bucket(bucket: &str) -> S3Error {
    S3Error(
        StatusCode::NOT_FOUND,
        "NoSuchBucket",
        "The specified bucket does not exist.".into(),
        format!("/{bucket}"),
    )
}

fn internal(err: anyhow::Error, resource: &str) -> S3Error {
    S3Error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "InternalError",
        format!("{err:#}"),
        resource.into(),
    )
}

fn invalid(message: impl Into<String>, resource: &str) -> S3Error {
    S3Error(
        StatusCode::BAD_REQUEST,
        "InvalidRequest",
        message.into(),
        resource.into(),
    )
}

// --- auth ------------------------------------------------------------------

/// Optional shared-secret check.
///
/// This is deliberately not SigV4. Request signing is out of scope for the v0 subset,
/// so the facade is expected to sit on loopback or another trusted interface. When a
/// token is configured it is required; when empty, everything is allowed.
fn check_auth(headers: &HeaderMap, st: &S3State, resource: &str) -> Result<(), S3Error> {
    if st.token.is_empty() {
        return Ok(());
    }
    let ok = headers
        .get("x-nexusfs-token")
        .and_then(|v| v.to_str().ok())
        .map(|t| constant_time_eq(t.as_bytes(), st.token.as_bytes()))
        .unwrap_or(false);

    if ok {
        Ok(())
    } else {
        Err(S3Error(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            "Access denied.".into(),
            resource.into(),
        ))
    }
}

/// Compare two secrets without leaking their common prefix through timing.
///
/// `==` on strings short-circuits at the first mismatch. Over a loopback interface that
/// is unlikely to be measurable, but the facade is not required to stay on loopback and
/// the fix costs nothing.
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

// --- key mapping -----------------------------------------------------------

fn check_bucket(bucket: &str, resource: &str) -> Result<(), S3Error> {
    if bucket.is_empty() || bucket.contains('/') || bucket == "." || bucket == ".." {
        return Err(invalid("invalid bucket name", resource));
    }
    Ok(())
}

/// Reject keys that would escape the bucket or confuse path resolution.
fn object_path(bucket: &str, key: &str) -> Result<String, S3Error> {
    let resource = format!("/{bucket}/{key}");
    check_bucket(bucket, &resource)?;

    if key.is_empty() {
        return Err(invalid("empty object key", &resource));
    }
    if key
        .split('/')
        .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(invalid(
            "object key must not contain empty or relative path segments",
            &resource,
        ));
    }
    Ok(format!("/{bucket}/{key}"))
}

/// NexusFS already hashes content, so the file's own object hash is the natural ETag.
///
/// Note this is BLAKE3, not the MD5 AWS returns. Clients that recompute an ETag to
/// verify an upload will disagree; clients that treat it as an opaque change token
/// work fine.
fn etag_for(core: &CoreState, path: &str) -> Option<String> {
    let (inode, _, _) = core.stat_file(path).ok()??;
    let record = core.load_inode(inode).ok()??;
    record.content.value.node_hash.map(hex::encode)
}

// --- handlers --------------------------------------------------------------

async fn list_buckets(State(st): State<S3State>, headers: HeaderMap) -> Result<Response, S3Error> {
    check_auth(&headers, &st, "/")?;

    let entries = st.core.read_dir_path("/").map_err(|e| internal(e, "/"))?;
    let buckets: Vec<(String, String)> = entries
        .into_iter()
        .filter(|e| e.entry_type == EntryType::Dir)
        .map(|e| {
            let created = st
                .core
                .load_inode(e.inode_id)
                .ok()
                .flatten()
                .map(|r| xml::iso8601(r.ctime_unix_ms))
                .unwrap_or_else(|| xml::iso8601(0));
            (e.name, created)
        })
        .collect();

    Ok((
        [(header::CONTENT_TYPE, "application/xml")],
        xml::list_buckets(&buckets),
    )
        .into_response())
}

async fn create_bucket(
    State(st): State<S3State>,
    Path(bucket): Path<String>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let resource = format!("/{bucket}");
    check_auth(&headers, &st, &resource)?;
    check_bucket(&bucket, &resource)?;

    st.core
        .mkdir_p(&st.identity, &resource, now_ms())
        .map_err(|e| internal(e, &resource))?;

    Ok((StatusCode::OK, [(header::LOCATION, resource.clone())]).into_response())
}

#[derive(Deserialize, Default)]
struct ListQuery {
    prefix: Option<String>,
    delimiter: Option<String>,
    #[serde(rename = "max-keys")]
    max_keys: Option<usize>,
    #[serde(rename = "continuation-token")]
    continuation_token: Option<String>,
}

async fn list_objects(
    State(st): State<S3State>,
    Path(bucket): Path<String>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let resource = format!("/{bucket}");
    check_auth(&headers, &st, &resource)?;
    check_bucket(&bucket, &resource)?;

    match st.core.resolve_path(&resource) {
        Ok(Some((_, EntryType::Dir))) => {}
        Ok(_) => return Err(no_such_bucket(&bucket)),
        Err(e) => return Err(internal(e, &resource)),
    }

    let walked = st
        .core
        .walk(&resource)
        .map_err(|e| internal(e, &resource))?;
    let prefix = q.prefix.unwrap_or_default();
    let max_keys = q.max_keys.unwrap_or(MAX_KEYS_LIMIT).min(MAX_KEYS_LIMIT);
    let strip = format!("{resource}/");

    // Directories are not objects in S3; they appear only as common prefixes.
    let mut keys: Vec<(String, u64, u64)> = walked
        .into_iter()
        .filter(|e| e.kind == EntryType::File)
        .map(|e| {
            let key = e.path.strip_prefix(&strip).unwrap_or(&e.path).to_string();
            (key, e.size, e.mtime_unix_ms)
        })
        .filter(|(k, _, _)| k.starts_with(&prefix))
        .collect();
    keys.sort_by(|a, b| a.0.cmp(&b.0));

    // Resume after the previous page's last key.
    if let Some(token) = q.continuation_token.as_deref() {
        keys.retain(|(k, _, _)| k.as_str() > token);
    }

    let mut objects = Vec::new();
    let mut common: Vec<String> = Vec::new();

    for (key, size, mtime) in &keys {
        if let Some(delim) = q.delimiter.as_deref().filter(|d| !d.is_empty()) {
            // Everything below the first delimiter after the prefix collapses into one
            // CommonPrefix — how S3 emulates folders over a flat keyspace.
            if let Some(idx) = key[prefix.len()..].find(delim) {
                let group = format!("{}{}", &key[..prefix.len() + idx], delim);
                if !common.contains(&group) {
                    common.push(group);
                }
                continue;
            }
        }

        objects.push(ObjectRow {
            key: key.clone(),
            size: *size,
            etag: etag_for(&st.core, &format!("{resource}/{key}")).unwrap_or_default(),
            last_modified: xml::iso8601(*mtime),
        });
    }

    let truncated = objects.len() + common.len() > max_keys;
    if truncated {
        objects.truncate(max_keys.min(objects.len()));
        common.truncate(max_keys.saturating_sub(objects.len()));
    }
    let next_token = if truncated {
        objects.last().map(|o| o.key.clone())
    } else {
        None
    };

    Ok((
        [(header::CONTENT_TYPE, "application/xml")],
        xml::list_objects_v2(
            &bucket,
            &prefix,
            q.delimiter.as_deref(),
            max_keys,
            truncated,
            next_token.as_deref(),
            &objects,
            &common,
        ),
    )
        .into_response())
}

async fn put_object(
    State(st): State<S3State>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, S3Error> {
    // Authenticate before validating the key: otherwise an anonymous caller can tell a
    // malformed key (400) from a rejected one (403) and probe the namespace shape.
    check_auth(&headers, &st, &format!("/{bucket}/{key}"))?;
    let path = object_path(&bucket, &key)?;

    st.core
        .write_file(&st.identity, &path, &body, now_ms())
        .map_err(|e| internal(e, &path))?;

    let etag = etag_for(&st.core, &path).unwrap_or_default();
    Ok((StatusCode::OK, [(header::ETAG, format!("\"{etag}\""))]).into_response())
}

async fn get_object(
    State(st): State<S3State>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let path = object_path(&bucket, &key)?;
    check_auth(&headers, &st, &path)?;

    let Some((_, _, mtime)) = st.core.stat_file(&path).map_err(|e| internal(e, &path))? else {
        return Err(no_such_key(&path));
    };
    let body = st
        .core
        .read_file_path(&path)
        .map_err(|e| internal(e, &path))?;
    let etag = etag_for(&st.core, &path).unwrap_or_default();

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::ETAG, format!("\"{etag}\"")),
            (header::LAST_MODIFIED, xml::iso8601(mtime)),
        ],
        body,
    )
        .into_response())
}

async fn head_object(
    State(st): State<S3State>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let path = object_path(&bucket, &key)?;
    check_auth(&headers, &st, &path)?;

    let Some((_, size, mtime)) = st.core.stat_file(&path).map_err(|e| internal(e, &path))? else {
        return Err(no_such_key(&path));
    };
    let etag = etag_for(&st.core, &path).unwrap_or_default();

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_LENGTH, size.to_string()),
            (header::ETAG, format!("\"{etag}\"")),
            (header::LAST_MODIFIED, xml::iso8601(mtime)),
        ],
    )
        .into_response())
}

async fn delete_object(
    State(st): State<S3State>,
    Path((bucket, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let path = object_path(&bucket, &key)?;
    check_auth(&headers, &st, &path)?;

    // S3 delete is idempotent: removing a key that is already gone succeeds.
    if st
        .core
        .stat_file(&path)
        .map_err(|e| internal(e, &path))?
        .is_some()
    {
        st.core
            .remove_path(&st.identity, &path, now_ms())
            .map_err(|e| internal(e, &path))?;
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}
