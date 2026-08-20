//! S3 facade tests.
//!
//! These drive the real router with real requests, so they cover routing, key
//! mapping and XML shape — not just the handler bodies.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use nexusfs_core::{CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_proto::DeviceId;
use nexusfs_s3::{routes, S3State};
use nexusfs_storage::mem_store::MemStore;

fn state(token: &str) -> S3State {
    let store = MemStore::new();
    let core = CoreState::new(
        Stores {
            blobs: Arc::new(store.clone()),
            kv: Arc::new(store),
        },
        DeviceId(1),
    );
    core.bootstrap_if_needed().unwrap();

    S3State {
        core: Arc::new(core),
        identity: Arc::new(Identity::from_seed([7u8; 32])),
        token: token.to_string(),
        // No peers in these tests: a missing chunk is genuinely missing.
        content: None,
    }
}

async fn send(st: &S3State, req: Request<Body>) -> (StatusCode, String) {
    let response = routes::router(st.clone()).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn put(uri: &str, body: &str) -> Request<Body> {
    Request::put(uri)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn put_then_get_round_trips() {
    let st = state("");

    let (status, _) = send(&st, put("/photos/holiday/beach.txt", "sand everywhere")).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(&st, get("/photos/holiday/beach.txt")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "sand everywhere");
}

#[tokio::test]
async fn put_creates_intermediate_directories() {
    // S3 keys are flat; the tree underneath has to be created implicitly.
    let st = state("");
    send(&st, put("/data/a/b/c/deep.txt", "found me")).await;

    let entries = st.core.read_dir_path("/data/a/b/c").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "deep.txt");
}

#[tokio::test]
async fn writes_go_through_the_operation_log() {
    // The whole point of the facade: it must not bypass oplog semantics.
    let st = state("");
    let before = st.core.op_count().unwrap();

    send(&st, put("/bucket/key.txt", "payload")).await;

    assert!(
        st.core.op_count().unwrap() > before,
        "an S3 PUT must append signed operations"
    );
    assert_eq!(st.core.pending_count().unwrap(), 0);
    // Every recorded operation carries a valid signature.
    for op in st.core.all_ops().unwrap() {
        st.core.verify_op(&op).unwrap();
    }
}

#[tokio::test]
async fn get_missing_key_is_404_with_s3_error_body() {
    let st = state("");
    let (status, body) = send(&st, get("/bucket/nope.txt")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("<Code>NoSuchKey</Code>"), "got: {body}");
}

#[tokio::test]
async fn head_reports_size_without_a_body() {
    let st = state("");
    send(&st, put("/b/file.bin", "0123456789")).await;

    let response = routes::router(st.clone())
        .oneshot(Request::head("/b/file.bin").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], "10");
    assert!(response.headers().contains_key("etag"));
}

#[tokio::test]
async fn delete_is_idempotent() {
    let st = state("");
    send(&st, put("/b/gone.txt", "x")).await;

    for _ in 0..2 {
        let response = routes::router(st.clone())
            .oneshot(Request::delete("/b/gone.txt").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    let (status, _) = send(&st, get("/b/gone.txt")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn overwrite_reuses_the_inode() {
    // Overwriting must not orphan the file's identity, or links to it would break.
    let st = state("");
    send(&st, put("/b/f.txt", "first")).await;
    let (inode_before, _, _) = st.core.stat_file("/b/f.txt").unwrap().unwrap();

    send(&st, put("/b/f.txt", "second")).await;
    let (inode_after, size, _) = st.core.stat_file("/b/f.txt").unwrap().unwrap();

    assert_eq!(inode_before, inode_after);
    assert_eq!(size, 6);
    let (_, body) = send(&st, get("/b/f.txt")).await;
    assert_eq!(body, "second");
}

#[tokio::test]
async fn list_objects_returns_keys_relative_to_the_bucket() {
    let st = state("");
    send(&st, put("/docs/a.txt", "1")).await;
    send(&st, put("/docs/sub/b.txt", "2")).await;

    let (status, body) = send(&st, get("/docs")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<Key>a.txt</Key>"), "got: {body}");
    assert!(body.contains("<Key>sub/b.txt</Key>"), "got: {body}");
    assert!(body.contains("<Name>docs</Name>"));
}

#[tokio::test]
async fn list_objects_honours_prefix_and_delimiter() {
    let st = state("");
    send(&st, put("/b/top.txt", "x")).await;
    send(&st, put("/b/dir/one.txt", "x")).await;
    send(&st, put("/b/dir/two.txt", "x")).await;

    let (_, body) = send(&st, get("/b?delimiter=%2F")).await;
    // With a delimiter, everything under dir/ collapses to one common prefix.
    assert!(body.contains("<Key>top.txt</Key>"), "got: {body}");
    assert!(body.contains("<Prefix>dir/</Prefix>"), "got: {body}");
    assert!(!body.contains("<Key>dir/one.txt</Key>"), "got: {body}");

    let (_, body) = send(&st, get("/b?prefix=dir%2F")).await;
    assert!(body.contains("<Key>dir/one.txt</Key>"), "got: {body}");
    assert!(!body.contains("<Key>top.txt</Key>"), "got: {body}");
}

#[tokio::test]
async fn list_objects_paginates() {
    let st = state("");
    for i in 0..5 {
        send(&st, put(&format!("/b/key{i}.txt"), "x")).await;
    }

    let (_, page) = send(&st, get("/b?max-keys=2")).await;
    assert!(
        page.contains("<IsTruncated>true</IsTruncated>"),
        "got: {page}"
    );
    assert!(
        page.contains("<NextContinuationToken>key1.txt<"),
        "got: {page}"
    );

    let (_, page2) = send(&st, get("/b?max-keys=2&continuation-token=key1.txt")).await;
    assert!(page2.contains("<Key>key2.txt</Key>"), "got: {page2}");
    assert!(!page2.contains("<Key>key0.txt</Key>"), "got: {page2}");
}

#[tokio::test]
async fn listing_a_missing_bucket_is_404() {
    let st = state("");
    let (status, body) = send(&st, get("/absent")).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("<Code>NoSuchBucket</Code>"), "got: {body}");
}

#[tokio::test]
async fn list_buckets_shows_top_level_directories() {
    let st = state("");
    send(&st, put("/alpha/x.txt", "1")).await;
    send(&st, put("/beta/y.txt", "2")).await;

    let (status, body) = send(&st, get("/")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<Name>alpha</Name>"), "got: {body}");
    assert!(body.contains("<Name>beta</Name>"), "got: {body}");
}

#[tokio::test]
async fn traversal_keys_are_rejected() {
    let st = state("");
    for key in ["/b/../escape.txt", "/b/./x.txt", "/b//double.txt"] {
        let (status, body) = send(&st, put(key, "x")).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "key {key} should be refused"
        );
        assert!(body.contains("<Code>InvalidRequest</Code>"));
    }
}

#[tokio::test]
async fn token_is_enforced_when_configured() {
    let st = state("secret");

    let (status, body) = send(&st, get("/any/key.txt")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("<Code>AccessDenied</Code>"));

    let authed = Request::put("/any/key.txt")
        .header("x-nexusfs-token", "secret")
        .body(Body::from("ok"))
        .unwrap();
    let (status, _) = send(&st, authed).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn keys_needing_xml_escaping_survive_a_listing() {
    let st = state("");
    send(&st, put("/b/a%26b.txt", "x")).await;

    let (_, body) = send(&st, get("/b")).await;
    assert!(body.contains("<Key>a&amp;b.txt</Key>"), "got: {body}");
}

#[tokio::test]
async fn binary_objects_round_trip_across_chunk_boundaries() {
    let st = state("");
    let payload: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();

    let request = Request::put("/b/big.bin")
        .body(Body::from(payload.clone()))
        .unwrap();
    let (status, _) = send(&st, request).await;
    assert_eq!(status, StatusCode::OK);

    let response = routes::router(st.clone())
        .oneshot(get("/b/big.bin"))
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.len(), payload.len());
    assert_eq!(body.as_ref(), payload.as_slice());
}

#[tokio::test]
async fn a_page_of_only_common_prefixes_still_returns_a_continuation_token() {
    // IsTruncated with no NextContinuationToken is malformed: the client is told there
    // is more but given no way to ask for it. Reachable whenever a truncated page
    // consists entirely of folder-style prefixes, because the token was taken from the
    // last *object* and there were none.
    let st = state("");
    for group in 1..=5 {
        let (status, _) = send(&st, put(&format!("/bk/g{group}/file.txt"), "x")).await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send(
        &st,
        Request::get("/bk?list-type=2&delimiter=/&max-keys=2")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<IsTruncated>true</IsTruncated>"), "{body}");
    assert!(
        body.contains("<NextContinuationToken>"),
        "a truncated page must say how to continue: {body}"
    );
}

#[tokio::test]
async fn paging_through_common_prefixes_does_not_repeat_them() {
    let st = state("");
    for group in 1..=5 {
        send(&st, put(&format!("/bk/g{group}/file.txt"), "x")).await;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..10 {
        let uri = match &token {
            Some(t) => format!("/bk?list-type=2&delimiter=/&max-keys=2&continuation-token={t}"),
            None => "/bk?list-type=2&delimiter=/&max-keys=2".to_string(),
        };
        let (_, body) = send(&st, Request::get(&uri).body(Body::empty()).unwrap()).await;

        for chunk in body.split("<Prefix>").skip(1) {
            if let Some(value) = chunk.split("</Prefix>").next() {
                if value.ends_with('/') {
                    seen.push(value.to_string());
                }
            }
        }

        if !body.contains("<IsTruncated>true</IsTruncated>") {
            token = None;
            break;
        }
        token = body
            .split("<NextContinuationToken>")
            .nth(1)
            .and_then(|c| c.split("</NextContinuationToken>").next())
            .map(|s| s.to_string());
        assert!(token.is_some(), "truncated page gave no token: {body}");
    }

    assert!(token.is_none(), "paging did not terminate");
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        seen.len(),
        unique.len(),
        "a prefix was returned twice: {seen:?}"
    );
    assert_eq!(
        unique.len(),
        5,
        "every group should appear exactly once: {unique:?}"
    );
}
