//! QUIC transport.
//!
//! # Why certificates are not the trust anchor
//!
//! Each node generates a self-signed certificate at startup and the client accepts
//! whatever certificate it is offered. That sounds alarming until you notice where
//! authentication actually happens: the Hello and HelloAck are signed with the device's
//! ed25519 key and pinned on first use (see `trust.rs`). TLS here provides transport
//! encryption and nothing else.
//!
//! The alternative — a shared CA, or distributing certificates out of band — would add
//! an enrolment step without improving on key pinning, which is the same trade
//! Syncthing and WireGuard make. What this design does *not* give you is protection
//! against an attacker who intercepts the very first connection to a new peer, before
//! any key is pinned. Enrol peers over a trusted path, or set `tofu = false` and pin
//! keys explicitly.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use quinn::{ClientConfig, Endpoint, ServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

/// Application-layer protocol negotiation identifier.
const ALPN: &[u8] = b"nexusfs/1";

/// rustls 0.23 requires the process to choose a crypto provider explicitly, and it
/// panics at first use if none was installed. Doing it here means a caller cannot
/// forget; `Once` makes the repeat calls harmless.
fn ensure_crypto_provider() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        // An error means another provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Accepts any server certificate.
///
/// Deliberate: peer identity is established by the signed handshake, not by PKI. See
/// the module docs before reaching for this elsewhere.
#[derive(Debug)]
struct PinnedByHandshake;

impl ServerCertVerifier for PinnedByHandshake {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ED25519,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PKCS1_SHA256,
        ]
    }

    fn root_hint_subjects(&self) -> Option<&[DistinguishedName]> {
        None
    }

    fn requires_raw_public_keys(&self) -> bool {
        false
    }
}

fn self_signed() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let cert = rcgen::generate_simple_self_signed(vec!["nexusfs".into()])
        .context("generate self-signed certificate")?;
    let der: CertificateDer<'static> = cert.cert.der().clone();
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()));
    Ok((der, key))
}

/// Endpoint that both listens for peers and can dial out.
pub fn endpoint(bind: SocketAddr) -> Result<Endpoint> {
    ensure_crypto_provider();
    let (cert, key) = self_signed()?;

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .context("build server TLS config")?;
    server_crypto.alpn_protocols = vec![ALPN.to_vec()];

    let server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
            .context("wrap server TLS config for QUIC")?,
    ));

    let mut endpoint = Endpoint::server(server_config, bind).context("bind QUIC endpoint")?;
    endpoint.set_default_client_config(client_config()?);
    Ok(endpoint)
}

fn client_config() -> Result<ClientConfig> {
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedByHandshake))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![ALPN.to_vec()];

    Ok(ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .context("wrap client TLS config for QUIC")?,
    )))
}

/// Dial a peer.
pub async fn connect(endpoint: &Endpoint, addr: SocketAddr) -> Result<quinn::Connection> {
    endpoint
        .connect(addr, "nexusfs")
        .context("start QUIC connection")?
        .await
        .context("complete QUIC handshake")
}
