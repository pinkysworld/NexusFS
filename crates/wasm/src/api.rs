//! JSON command surface exposed to the host page.

use std::cell::RefCell;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use nexusfs_core::{decode, encode, EntryType};
use nexusfs_proto::FsOp;

use crate::replica::Replica;

thread_local! {
    static REPLICAS: RefCell<Vec<Replica>> = const { RefCell::new(Vec::new()) };
}

#[derive(Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    replica: usize,
    #[serde(default)]
    now: u64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    payload: Value,
}

#[derive(Serialize, Deserialize)]
struct SyncPayload {
    ops: Vec<String>,
    blobs: Vec<(String, String)>,
}

pub fn dispatch(request: &[u8]) -> Vec<u8> {
    let response = match run(request) {
        Ok(value) => json!({ "ok": true, "data": value }),
        Err(err) => json!({ "ok": false, "error": format!("{err:#}") }),
    };
    serde_json::to_vec(&response)
        .unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"encode\"}".to_vec())
}

fn run(request: &[u8]) -> Result<Value> {
    let req: Request = serde_json::from_slice(request).context("parse request")?;

    match req.op.as_str() {
        "reset" => reset(&req.names),
        "mkdir" => with(req.replica, |r| r.mkdir(&req.path, req.now)).map(|_| Value::Null),
        "put" => with(req.replica, |r| {
            r.put(&req.path, req.content.as_bytes(), req.now)
        })
        .map(|_| Value::Null),
        "rm" => with(req.replica, |r| r.rm(&req.path, req.now)).map(|_| Value::Null),
        "mv" => with(req.replica, |r| r.mv(&req.from, &req.to, req.now)).map(|_| Value::Null),
        "cat" => with(req.replica, |r| {
            let bytes = r.core.read_file_path(&req.path)?;
            Ok(json!(String::from_utf8_lossy(&bytes)))
        }),
        "tree" => with(req.replica, |r| tree(r, "/")),
        "state" => with(req.replica, state),
        "export" => with(req.replica, export),
        "import" => import(req.replica, req.payload),
        other => bail!("unknown command: {other}"),
    }
}

fn with<T>(index: usize, f: impl FnOnce(&Replica) -> Result<T>) -> Result<T> {
    REPLICAS.with(|slot| {
        let replicas = slot.borrow();
        let replica = replicas
            .get(index)
            .with_context(|| format!("no replica {index}"))?;
        f(replica)
    })
}

/// Create a fresh set of replicas, discarding any existing ones.
fn reset(names: &[String]) -> Result<Value> {
    let names: Vec<String> = if names.is_empty() {
        vec!["A".into(), "B".into()]
    } else {
        names.to_vec()
    };

    let mut replicas = Vec::new();
    for (i, name) in names.iter().enumerate() {
        // Distinct, stable device ids and key seeds per replica. Deterministic on
        // purpose: the demo should behave the same on every reload.
        let device_id = (i as u128) + 1;
        let mut seed = [0u8; 32];
        seed[0] = (i as u8) + 1;
        replicas.push(Replica::new(name.clone(), device_id, seed)?);
    }

    let summary: Vec<Value> = replicas
        .iter()
        .enumerate()
        .map(|(i, r)| json!({ "index": i, "name": r.name }))
        .collect();

    REPLICAS.with(|slot| *slot.borrow_mut() = replicas);
    Ok(json!(summary))
}

fn state(r: &Replica) -> Result<Value> {
    let (blob_count, blob_bytes) = r.core.blob_stats()?;
    Ok(json!({
        "name": r.name,
        "device_id": format!("{:x}", r.core.device_id.0),
        "state_root": hex::encode(r.core.compute_state_root()?),
        "head": r.core.get_head()?.map(hex::encode),
        "ops": r.core.op_count()?,
        "applied": r.core.applied_count()?,
        "pending": r.core.pending_count()?,
        "blob_count": blob_count,
        "blob_bytes": blob_bytes,
    }))
}

/// Depth-first listing, so the UI can render the whole namespace at once.
fn tree(r: &Replica, path: &str) -> Result<Value> {
    let mut out = Vec::new();
    walk(r, path, 0, &mut out)?;
    Ok(json!(out))
}

fn walk(r: &Replica, path: &str, depth: usize, out: &mut Vec<Value>) -> Result<()> {
    if depth > 32 {
        return Ok(());
    }
    for entry in r.core.read_dir_path(path)? {
        let child = if path == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{path}/{}", entry.name)
        };

        match entry.entry_type {
            EntryType::Dir => {
                out.push(json!({
                    "path": child, "name": entry.name, "kind": "dir", "depth": depth, "size": 0,
                }));
                walk(r, &child, depth + 1, out)?;
            }
            EntryType::File => {
                let size = r
                    .core
                    .load_inode(entry.inode_id)?
                    .map(|rec| rec.content.value.size)
                    .unwrap_or(0);
                out.push(json!({
                    "path": child, "name": entry.name, "kind": "file", "depth": depth, "size": size,
                }));
            }
        }
    }
    Ok(())
}

fn export(r: &Replica) -> Result<Value> {
    let (ops, blobs) = r.export()?;
    let payload = SyncPayload {
        ops: ops
            .iter()
            .map(|op| encode(op).map(|bytes| B64.encode(bytes)))
            .collect::<Result<Vec<_>>>()?,
        blobs: blobs
            .into_iter()
            .map(|(h, v)| (hex::encode(h), B64.encode(v)))
            .collect(),
    };
    Ok(serde_json::to_value(payload)?)
}

fn import(index: usize, payload: Value) -> Result<Value> {
    let payload: SyncPayload = serde_json::from_value(payload).context("parse sync payload")?;

    let mut ops = Vec::new();
    for encoded in &payload.ops {
        let bytes = B64.decode(encoded).context("decode op")?;
        ops.push(decode::<FsOp>(&bytes).context("decode op")?);
    }

    let mut blobs = Vec::new();
    for (hash_hex, encoded) in &payload.blobs {
        let raw = hex::decode(hash_hex).context("decode blob hash")?;
        let hash: [u8; 32] = raw
            .try_into()
            .map_err(|_| anyhow::anyhow!("blob hash is not 32 bytes"))?;
        blobs.push((hash, B64.decode(encoded).context("decode blob")?));
    }

    let applied = with(index, |r| r.import(ops, blobs))?;
    Ok(json!({ "applied": applied }))
}
