use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::RootCertStore;

/// Development-only QUIC endpoint creation with a self-signed certificate.
///
/// Production should use a real PKI, allowlists, or out-of-band pairing.
pub fn make_dev_endpoint(listen: SocketAddr) -> Result<Endpoint> {
    // Generate a self-signed cert for localhost. In real deployments, use proper identities.
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .context("rcgen generate cert")?;
    let cert_der: CertificateDer<'static> = cert.cert.der().clone();
    let key_der: PrivateKeyDer<'static> =
        PrivateKeyDer::from(PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()));

    // Server config
    let server_config = ServerConfig::with_single_cert(vec![cert_der.clone()], key_der)
        .context("create server config")?;

    let mut endpoint =
        Endpoint::server(server_config, listen).context("create quic server endpoint")?;

    // Client config trusts our self-signed cert (dev only).
    let mut roots = RootCertStore::empty();
    roots.add(cert_der).context("add root cert")?;

    let client_config =
        ClientConfig::with_root_certificates(Arc::new(roots)).context("create client config")?;
    endpoint.set_default_client_config(client_config);

    Ok(endpoint)
}

/// Connect to a peer address.
pub async fn connect(
    endpoint: &Endpoint,
    addr: SocketAddr,
    server_name: &str,
) -> Result<quinn::Connection> {
    let conn = endpoint
        .connect(addr, server_name)
        .context("connect call failed")?
        .await
        .context("connect await failed")?;
    Ok(conn)
}
