use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use nexusfs_core::{CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_storage::sled_store::SledStore;

use crate::config::Config;

/// Bridges the replication registry to the admin API without either crate depending
/// on the other.
#[cfg(all(feature = "admin", feature = "quic"))]
struct PeerBridge(nexusfs_net::peers::PeerRegistry);

#[cfg(all(feature = "admin", feature = "quic"))]
impl nexusfs_admin::PeerSource for PeerBridge {
    fn peers(&self) -> Vec<nexusfs_admin::PeerView> {
        self.0
            .snapshot()
            .into_iter()
            .map(|p| nexusfs_admin::PeerView {
                address: p.address,
                device_id: p.device_id,
                last_attempt_ms: p.last_attempt_ms,
                last_success_ms: p.last_success_ms,
                last_error: p.last_error,
                ops_received: p.ops_received,
                blobs_received: p.blobs_received,
                syncs: p.syncs,
            })
            .collect()
    }
}

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
    let (core, identity, admin_token) = open_core(&cfg)?;

    // Bootstrap local repo if empty.
    core.bootstrap_if_needed()?;

    // Shared so the admin API can report sync state while syncs are in flight.
    #[cfg(feature = "quic")]
    let peer_registry = nexusfs_net::peers::PeerRegistry::new();

    // Start admin server.
    #[cfg(feature = "admin")]
    {
        let addr = cfg.admin_addr()?;

        #[cfg(feature = "quic")]
        let peers: Option<Arc<dyn nexusfs_admin::PeerSource>> =
            Some(Arc::new(PeerBridge(peer_registry.clone())));
        #[cfg(not(feature = "quic"))]
        let peers: Option<Arc<dyn nexusfs_admin::PeerSource>> = None;

        let st = nexusfs_admin::AdminState {
            core: Arc::new(core.clone()),
            token: admin_token.clone(),
            peers,
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
            // Object writes are signed by this node, exactly like CLI writes.
            identity: Arc::new(identity.clone()),
            token: cfg.s3.token.clone(),
        };
        if st.token.is_empty() {
            tracing::warn!(
                "s3 facade has no token configured; anyone who can reach {addr} can read \
                 and write objects"
            );
        }
        tokio::spawn(async move {
            if let Err(e) = nexusfs_s3::serve(addr, st).await {
                eprintln!("s3 server error: {e:?}");
            }
        });
    }

    // Start replication: serve inbound sessions, and pull from configured peers.
    #[cfg(feature = "quic")]
    {
        let listen = cfg.net_addr()?;
        let ctx = nexusfs_net::session::SessionCtx {
            core: core.clone(),
            identity: identity.clone(),
            device_id: core.device_id,
            trust: nexusfs_net::trust::TrustPolicy { tofu: cfg.net.tofu },
        };

        match nexusfs_net::quic::endpoint(listen) {
            Ok(endpoint) => {
                info!(%listen, peers = cfg.net.peers.len(), "replication enabled");
                if cfg.net.tofu {
                    tracing::warn!(
                        "trust-on-first-use is enabled; the first connection from an \
                         unknown device will be trusted and its key pinned"
                    );
                }

                tokio::spawn(nexusfs_net::peers::accept_loop(
                    endpoint.clone(),
                    ctx.clone(),
                ));
                tokio::spawn(nexusfs_net::peers::sync_loop(
                    endpoint,
                    cfg.net.peers.clone(),
                    ctx,
                    std::time::Duration::from_secs(cfg.net.sync_interval_secs.max(1)),
                    peer_registry.clone(),
                ));
            }
            Err(e) => tracing::error!(error = %format!("{e:#}"), "replication failed to start"),
        }
    }

    // Identity signs operations for the S3 facade and the replication handshake; with
    // neither feature the daemon merely holds it open.
    #[cfg(not(any(feature = "s3", feature = "quic")))]
    let _ = &identity;

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
