use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use nexusfs_core::{CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_storage::sled_store::SledStore;

use crate::config::Config;

pub async fn run_status(config_path: PathBuf) -> Result<()> {
    let cfg = Config::load(&config_path)?;
    let (core, _identity, admin_token) = open_core(&cfg)?;
    core.bootstrap_if_needed()?;

    let (blob_count, blob_bytes) = core.blob_stats()?;

    println!("device_id:   {:x}", core.device_id.0);
    println!("data_dir:    {}", cfg.data_dir().display());
    println!(
        "head:        {}",
        core.get_head()?
            .map(hex::encode)
            .unwrap_or_else(|| "(none)".into())
    );
    println!(
        "state_root:  {}",
        core.get_state_root()?
            .map(hex::encode)
            .unwrap_or_else(|| "(none)".into())
    );
    println!(
        "ops:         {} ({} applied)",
        core.op_count()?,
        core.applied_count()?
    );
    println!("pending:     {}", core.pending_count()?);
    println!("blobs:       {blob_count} ({blob_bytes} bytes)");
    println!(
        "admin_token: {}",
        if admin_token.is_empty() {
            "(dev mode / empty)".into()
        } else {
            admin_token
        }
    );
    Ok(())
}

pub async fn run_daemon(config_path: PathBuf) -> Result<()> {
    let cfg = Config::load(&config_path)?;
    let (core, _identity, admin_token) = open_core(&cfg)?;

    // Bootstrap local repo if empty.
    core.bootstrap_if_needed()?;

    // Start admin server.
    #[cfg(feature = "admin")]
    {
        let addr = cfg.admin_addr()?;
        let st = nexusfs_admin::AdminState {
            core: Arc::new(core.clone()),
            token: admin_token.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = nexusfs_admin::serve(addr, st).await {
                eprintln!("admin server error: {e:?}");
            }
        });
    }
    #[cfg(not(feature = "admin"))]
    {
        tracing::warn!("admin feature disabled; no admin server will run");
    }

    // Start S3 server if enabled and feature is present.
    #[cfg(feature = "s3")]
    if cfg.s3.enabled {
        let addr = cfg.s3_addr()?;
        let st = nexusfs_s3::S3State {
            core: Arc::new(core.clone()),
        };
        tokio::spawn(async move {
            if let Err(e) = nexusfs_s3::serve(addr, st).await {
                eprintln!("s3 server error: {e:?}");
            }
        });
    }

    // TODO(feature=quic): start replication manager, peer connections, etc.

    info!("nexusfs daemon running. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    info!("shutdown requested");
    Ok(())
}

pub(crate) fn open_core(cfg: &Config) -> Result<(CoreState, Identity, String)> {
    // Open local DB (sled default).
    let data_dir = cfg.data_dir();
    std::fs::create_dir_all(&data_dir).ok();

    let db_path = data_dir.join("db");
    let store = SledStore::open(&db_path).context("open sled store")?;
    let stores = Stores {
        blobs: Arc::new(store.clone()),
        kv: Arc::new(store),
    };

    // Load/generate identity.
    let id_path = data_dir.join("identity.toml");
    let identity = Identity::load_or_create(&id_path).context("load or create identity")?;

    // Device id: store a persistent random u128 in KV if absent.
    let device_id = load_or_create_device_id(&stores)?;
    let core = CoreState::new(stores, device_id);

    // Admin token:
    // - if config token provided, use it
    // - else load from KV or generate new and store
    let admin_token = if !cfg.admin.token.is_empty() {
        cfg.admin.token.clone()
    } else {
        load_or_create_admin_token(&core)?
    };

    Ok((core, identity, admin_token))
}

fn load_or_create_device_id(stores: &Stores) -> Result<nexusfs_proto::DeviceId> {
    const CF: &str = "meta";
    const KEY: &[u8] = b"node/device_id";
    if let Some(v) = stores.kv.get_kv(CF, KEY)? {
        if v.len() == 16 {
            let mut b = [0u8; 16];
            b.copy_from_slice(&v);
            return Ok(nexusfs_proto::DeviceId(u128::from_be_bytes(b)));
        }
    }
    let did: u128 = rand::random();
    stores.kv.put_kv(CF, KEY, &did.to_be_bytes())?;
    Ok(nexusfs_proto::DeviceId(did))
}

fn load_or_create_admin_token(core: &CoreState) -> Result<String> {
    const CF: &str = "meta";
    const KEY: &[u8] = b"admin/token";
    if let Some(v) = core.stores.kv.get_kv(CF, KEY)? {
        if let Ok(s) = String::from_utf8(v) {
            if !s.is_empty() {
                return Ok(s);
            }
        }
    }
    // Generate a random token.
    let token = random_token(32);
    core.stores.kv.put_kv(CF, KEY, token.as_bytes())?;
    Ok(token)
}

fn random_token(len: usize) -> String {
    const ALPH: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let i = (rand::random::<u32>() as usize) % ALPH.len();
        out.push(ALPH[i] as char);
    }
    out
}
