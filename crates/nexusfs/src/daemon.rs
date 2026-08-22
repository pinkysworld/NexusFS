use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use nexusfs_core::{CoreState, Stores};
use nexusfs_crypto::Identity;
use nexusfs_storage::sled_store::SledStore;

use crate::config::Config;

/// Samples the device, asks the scheduler what that permits, and caches the answer.
///
/// One instance serves two consumers that must not disagree: replication reads it
/// through `SyncGate` to decide what to transfer, and the admin console reads it
/// through `EnergySource` to explain what is happening. Sampling once and caching is
/// what keeps the console's explanation true of the pass that actually ran — a second
/// independent sample could report "on mains" next to a throttled sync.
#[cfg(any(feature = "quic", feature = "admin"))]
struct EnergyGate {
    scheduler: nexusfs_energy::RuleBasedScheduler,
    /// Reported by the console so an operator can tell "nothing is throttling this"
    /// apart from "throttling is switched off"; nothing else needs it.
    #[cfg_attr(not(feature = "admin"), allow(dead_code))]
    enabled: bool,
    core: CoreState,
    /// Link cost stated in config; `None` means detect it each pass.
    link_override: Option<nexusfs_energy::LinkCost>,
    /// The store's directory, whose free space is the one that matters — a node with
    /// its store on an external volume does not care what `/` has left.
    data_dir: PathBuf,
    last: std::sync::Mutex<(nexusfs_energy::Telemetry, nexusfs_energy::SyncBudget)>,
}

#[cfg(any(feature = "quic", feature = "admin"))]
impl EnergyGate {
    fn new(cfg: &crate::config::Energy, core: CoreState, data_dir: PathBuf) -> Self {
        let thresholds = nexusfs_energy::Thresholds::from_config(
            cfg.battery_low_pct,
            cfg.temp_high_c,
            cfg.storage_reserve_mb,
        );
        let scheduler = if cfg.enabled {
            nexusfs_energy::RuleBasedScheduler::new(thresholds)
        } else {
            nexusfs_energy::RuleBasedScheduler::disabled()
        };

        // A value nobody recognises must not quietly become a constraint, nor quietly
        // become "auto" in silence — the operator wrote it meaning something.
        if !nexusfs_energy::link::config_is_valid(&cfg.link_cost) {
            tracing::warn!(
                value = %cfg.link_cost,
                "energy.link_cost is not one of auto, metered, unmetered or unknown; \
                 detecting instead"
            );
        }
        let link_override = nexusfs_energy::link::parse_config(&cfg.link_cost);
        if let Some(link) = link_override {
            tracing::info!(?link, "link cost is set in config; skipping detection");
        }

        Self {
            scheduler,
            enabled: cfg.enabled,
            core,
            link_override,
            data_dir,
            last: std::sync::Mutex::new((
                nexusfs_energy::Telemetry::default(),
                nexusfs_energy::SyncBudget::unlimited(),
            )),
        }
    }

    fn sample(&self) -> (nexusfs_energy::Telemetry, nexusfs_energy::SyncBudget) {
        use nexusfs_energy::Scheduler as _;

        let telemetry = nexusfs_energy::telemetry::sample_with(&nexusfs_energy::SampleInputs {
            link: self.link_override,
            data_dir: Some(&self.data_dir),
        });
        // Backlog size feeds the conserving band: a device with nothing outstanding has
        // no reason to be capped.
        let backlog = nexusfs_energy::BacklogView {
            pending_ops: self.core.pending_count().unwrap_or(0) as u64,
            missing_chunks: self
                .core
                .missing_chunk_hashes()
                .map(|h| h.len() as u64)
                .unwrap_or(0),
        };
        let budget = self.scheduler.plan(&telemetry, &backlog);

        *self.last.lock().expect("energy gate poisoned") = (telemetry.clone(), budget.clone());
        (telemetry, budget)
    }

    /// The reading to report, preferring the one replication last acted on.
    ///
    /// Falls back to sampling when that reading is missing or older than
    /// [`STALE_AFTER_MS`], which is what keeps the console live on a node with
    /// replication switched off — there, nothing else ever calls `sample`.
    #[cfg(feature = "admin")]
    fn current(&self) -> (nexusfs_energy::Telemetry, nexusfs_energy::SyncBudget) {
        /// Comfortably longer than any sane sync interval, so a node that *is*
        /// replicating always reports the decision behind the last pass rather than a
        /// fresh sample that could contradict it.
        const STALE_AFTER_MS: u64 = 60_000;

        {
            let cached = self.last.lock().expect("energy gate poisoned");
            let age = nexusfs_core::now_ms().saturating_sub(cached.0.sampled_unix_ms);
            if cached.0.sampled_unix_ms != 0 && age < STALE_AFTER_MS {
                return cached.clone();
            }
        }
        self.sample()
    }
}

#[cfg(feature = "quic")]
impl nexusfs_net::session::SyncGate for EnergyGate {
    fn decide(&self) -> nexusfs_net::session::SyncDecision {
        let (_, budget) = self.sample();
        nexusfs_net::session::SyncDecision {
            sync: budget.sync,
            limits: nexusfs_net::session::SyncLimits {
                content: budget.content,
                max_content_bytes: budget.max_content_bytes,
            },
            interval_scale: budget.interval_scale,
            reason: budget.reason,
        }
    }
}

#[cfg(feature = "admin")]
impl nexusfs_admin::EnergySource for EnergyGate {
    fn energy(&self) -> nexusfs_admin::EnergyView {
        use nexusfs_energy::{LinkCost, PowerSource};

        let (t, b) = self.current();

        nexusfs_admin::EnergyView {
            enabled: self.enabled,
            power: match t.power {
                PowerSource::Mains => "mains",
                PowerSource::Battery => "battery",
                PowerSource::Unknown => "unknown",
            }
            .into(),
            battery_pct: t.battery_pct,
            temp_c: t.temp_c,
            cpu_load: t.cpu_load,
            link: match t.link {
                LinkCost::Unmetered => "unmetered",
                LinkCost::Metered => "metered",
                LinkCost::Unknown => "unknown",
            }
            .into(),
            storage_free_bytes: t.storage_free_bytes,
            sampled_unix_ms: t.sampled_unix_ms,
            sync: b.sync,
            content: b.content,
            max_content_bytes: (b.max_content_bytes != u64::MAX).then_some(b.max_content_bytes),
            interval_scale: b.interval_scale,
            reason: b.reason,
        }
    }
}

/// Fetches deferred content from peers on behalf of a read.
///
/// The counterpart to the energy scheduler's decision to take operations and skip
/// bytes. Without this, a node under a metadata-only budget knows a file exists and
/// cannot serve it until the next unconstrained pass — which turns a deliberate policy
/// into an outage from the reader's point of view.
#[cfg(feature = "quic")]
struct PeerContentFetcher {
    endpoint: nexusfs_net::quic::QuicEndpoint,
    peers: Vec<String>,
    ctx: nexusfs_net::session::SessionCtx,
}

#[cfg(feature = "quic")]
impl nexusfs_core::ContentFetcher for PeerContentFetcher {
    fn fetch<'a>(&'a self, hashes: &'a [nexusfs_proto::Hash]) -> nexusfs_core::FetchFuture<'a> {
        Box::pin(async move {
            Ok(
                nexusfs_net::peers::fetch_from_peers(
                    &self.endpoint,
                    &self.peers,
                    &self.ctx,
                    hashes,
                )
                .await,
            )
        })
    }
}

/// Carries "this node just wrote something" from the apply path to the task that tells
/// peers about it.
///
/// A channel rather than the notifying itself, because `applied` runs on the apply path
/// with somebody waiting on it: opening a QUIC connection there would put a handshake
/// between a user and their `put` returning.
#[cfg(feature = "quic")]
struct LocalOpChannel(tokio::sync::mpsc::Sender<()>);

#[cfg(feature = "quic")]
impl nexusfs_core::LocalOpSink for LocalOpChannel {
    fn applied(&self) {
        // `try_send`, never `send`. A full channel already means a notification is
        // pending, and a second one would say nothing the first does not — so dropping
        // it is not a loss, it is the coalescing working.
        let _ = self.0.try_send(());
    }
}

/// How long to wait after a local write before telling peers.
///
/// One `put` is several operations — a `mkdir`, a `CreateFile`, a `Write` — and each
/// would otherwise be its own round of handshakes to every peer. Long enough to collect
/// those into one notification, short enough that nobody perceives it.
#[cfg(feature = "quic")]
const NOTIFY_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

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
                content_bytes: p.content_bytes,
                content_deferred: p.content_deferred,
                syncs: p.syncs,
            })
            .collect()
    }
}

pub async fn run_status(config_path: PathBuf) -> Result<()> {
    let cfg = Config::load(&config_path)?;
    let (core, _identity, admin_token) = open_core(&cfg)?;
    core.require_current_format()?;
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
    print_energy(&cfg, &core);
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

/// Print the current power reading and what replication may do about it.
///
/// The same decision `/api/energy` serves, in the one place an operator can reach
/// without the admin feature compiled in — which is exactly the build most likely to be
/// running on a constrained device, and so the one most likely to be throttling. A node
/// that silently syncs less than expected and cannot say why is a support ticket.
///
/// Reported even when scheduling is disabled, because "nothing is throttling this" and
/// "throttling is switched off" are different answers to the same question.
fn print_energy(cfg: &Config, core: &CoreState) {
    use nexusfs_energy::Scheduler as _;

    let thresholds = nexusfs_energy::Thresholds::from_config(
        cfg.energy.battery_low_pct,
        cfg.energy.temp_high_c,
        cfg.energy.storage_reserve_mb,
    );
    let scheduler = if cfg.energy.enabled {
        nexusfs_energy::RuleBasedScheduler::new(thresholds)
    } else {
        nexusfs_energy::RuleBasedScheduler::disabled()
    };

    let data_dir = cfg.data_dir();
    let telemetry = nexusfs_energy::telemetry::sample_with(&nexusfs_energy::SampleInputs {
        link: nexusfs_energy::link::parse_config(&cfg.energy.link_cost),
        data_dir: Some(&data_dir),
    });
    let backlog = nexusfs_energy::BacklogView {
        pending_ops: core.pending_count().unwrap_or(0) as u64,
        missing_chunks: core
            .missing_chunk_hashes()
            .map(|h| h.len() as u64)
            .unwrap_or(0),
    };
    let budget = scheduler.plan(&telemetry, &backlog);

    let power = match telemetry.power {
        nexusfs_energy::PowerSource::Mains => "mains".to_string(),
        nexusfs_energy::PowerSource::Battery => match telemetry.battery_pct {
            Some(pct) => format!("battery {pct}%"),
            None => "battery (charge unreadable)".into(),
        },
        nexusfs_energy::PowerSource::Unknown => "unknown".into(),
    };
    let link = match telemetry.link {
        nexusfs_energy::LinkCost::Metered => "metered",
        nexusfs_energy::LinkCost::Unmetered => "unmetered",
        nexusfs_energy::LinkCost::Unknown => "unknown",
    };

    println!("power:       {power}, link {link}");
    let allowed = if !budget.sync {
        "paused".to_string()
    } else if !budget.content {
        "operations only".into()
    } else if budget.max_content_bytes == u64::MAX {
        "operations and content".into()
    } else {
        format!(
            "operations and up to {} of content per pass",
            nexusfs_energy::human_bytes(budget.max_content_bytes)
        )
    };
    println!(
        "sync budget: {allowed}{}",
        if cfg.energy.enabled {
            String::new()
        } else {
            " (energy-aware scheduling is disabled)".into()
        }
    );
    println!("             {}", budget.reason);
}

pub async fn run_daemon(config_path: PathBuf) -> Result<()> {
    let cfg = Config::load(&config_path)?;
    // The token is only read by the admin console; a build without it still needs one
    // minted so the on-disk state is the same whichever build touches it.
    #[cfg_attr(not(feature = "admin"), allow(unused_variables))]
    let (core, identity, admin_token) = open_core(&cfg)?;

    // Attached before anything clones `core`, so every path that applies an operation
    // reports it — the CLI-facing facades included, not only the sync loop.
    #[cfg(feature = "quic")]
    let (local_ops_tx, mut local_ops_rx) = tokio::sync::mpsc::channel::<()>(1);
    #[cfg(feature = "quic")]
    let core = core.with_local_op_sink(Arc::new(LocalOpChannel(local_ops_tx)));

    // Before anything is written: an old or newer store must not be operated on by a
    // build that does not match it.
    core.require_current_format()?;

    // Bootstrap local repo if empty.
    core.bootstrap_if_needed()?;

    // Shared so the admin API can report sync state while syncs are in flight.
    #[cfg(feature = "quic")]
    let peer_registry = nexusfs_net::peers::PeerRegistry::new();

    // One gate, two readers: replication acts on it, the console explains it.
    #[cfg(any(feature = "quic", feature = "admin"))]
    let energy_gate = {
        if !cfg.energy.enabled {
            info!("energy-aware scheduling is disabled; replication will run unthrottled");
        }
        Arc::new(EnergyGate::new(&cfg.energy, core.clone(), cfg.data_dir()))
    };

    // Replication starts before the facades, because they borrow its transport to
    // fetch content this node deferred. Starting them first would mean either a second
    // endpoint or a read path that cannot reach a peer.
    // Consumed by the facades; a quic-only build still runs replication, it just has
    // no reader to serve deferred content to.
    #[cfg_attr(not(any(feature = "admin", feature = "s3")), allow(unused_variables))]
    #[cfg(feature = "quic")]
    let content_fetcher: Option<Arc<dyn nexusfs_core::ContentFetcher>> = 'replication: {
        // Not `?`: replication starting before the facades means a typo in `net.listen`
        // would otherwise abort the daemon before the admin console is up — and a wrong
        // config is exactly when an operator wants that console.
        let listen = match cfg.net_addr() {
            Ok(listen) => listen,
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "replication failed to start");
                break 'replication None;
            }
        };
        // Shared between the sessions that receive notifications and the loop that
        // acts on them.
        let wake = Arc::new(tokio::sync::Notify::new());
        let ctx = nexusfs_net::session::SessionCtx {
            core: core.clone(),
            identity: identity.clone(),
            device_id: core.device_id,
            trust: nexusfs_net::trust::TrustPolicy { tofu: cfg.net.tofu },
            wake: Some(wake.clone()),
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

                // Tell peers when this node writes, so they do not wait out a poll
                // interval to find out. Configured peers only: this node knows who it
                // pulls from, not who pulls from it, so a one-directional pairing falls
                // back to polling on the side that was never told about.
                tokio::spawn({
                    let endpoint = endpoint.clone();
                    let peers = cfg.net.peers.clone();
                    let ctx = ctx.clone();
                    async move {
                        while local_ops_rx.recv().await.is_some() {
                            tokio::time::sleep(NOTIFY_DEBOUNCE).await;
                            // Drain whatever else arrived while waiting; they are all
                            // the same message.
                            while local_ops_rx.try_recv().is_ok() {}
                            nexusfs_net::peers::notify_peers(&endpoint, &peers, &ctx).await;
                        }
                    }
                });
                tokio::spawn(nexusfs_net::peers::sync_loop(
                    endpoint.clone(),
                    cfg.net.peers.clone(),
                    ctx.clone(),
                    std::time::Duration::from_secs(cfg.net.sync_interval_secs.max(1)),
                    peer_registry.clone(),
                    Some(energy_gate.clone() as Arc<dyn nexusfs_net::session::SyncGate>),
                ));

                Some(Arc::new(PeerContentFetcher {
                    endpoint,
                    peers: cfg.net.peers.clone(),
                    ctx,
                }) as Arc<dyn nexusfs_core::ContentFetcher>)
            }
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "replication failed to start");
                None
            }
        }
    };
    #[cfg_attr(not(any(feature = "admin", feature = "s3")), allow(unused_variables))]
    #[cfg(not(feature = "quic"))]
    let content_fetcher: Option<Arc<dyn nexusfs_core::ContentFetcher>> = None;

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
            energy: Some(energy_gate.clone() as Arc<dyn nexusfs_admin::EnergySource>),
            content: content_fetcher.clone(),
            node_pubkey: Some(identity.pubkey_bytes()),
            node_seal_key: Some(identity.sealing_pubkey()),
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
            content: content_fetcher.clone(),
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
    let stores = Stores::shared(store);

    // Load/generate identity.
    let id_path = data_dir.join("identity.toml");
    let identity = Identity::load_or_create(&id_path).context("load or create identity")?;

    // Device id: store a persistent random u128 in KV if absent.
    let device_id = load_or_create_device_id(&stores)?;
    // The sealing key is supplied unconditionally, not only when encryption is on: it
    // is what opens content *others* sealed to this device, and a node that replicates
    // an encrypted file from a peer must be able to read it whether or not it would
    // have encrypted its own writes.
    let mut core = CoreState::new(stores, device_id).with_sealing_key(identity.sealing_secret());

    // At-rest encryption. New writes seal the file key to each enrolled recipient, so
    // the repository key is needed only to read files written before that existed —
    // it is still loaded, because those files must keep working.
    if cfg.security.encrypt_at_rest {
        let key_path = data_dir.join("repo.key");
        let cipher = nexusfs_crypto::RepoCipher::load_or_create(&key_path)
            .context("load or create repository key")?;
        core = core.with_encryption(Arc::new(cipher));
    }

    let policy = nexusfs_core::ProofPolicy::from_config(&cfg.security.proof_mode);
    if policy == nexusfs_core::ProofPolicy::None
        && !matches!(
            cfg.security.proof_mode.trim().to_ascii_lowercase().as_str(),
            "" | "none"
        )
    {
        tracing::warn!(
            mode = %cfg.security.proof_mode,
            "proof mode is not implemented in this build; running without proofs"
        );
    }
    let core = core.with_proofs(policy);

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
