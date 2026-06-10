//! Hardened TLS client/server config.
//!
//! Lessons from RustDesk CVE-2026-30794:
//!   The vulnerable code path in their client used a `danger_accept_invalid_certs`
//!   fallback when the initial TLS handshake failed — allowing AiTM with a fake cert.
//!
//! Our policy: **never** disable certificate validation. If the cert chain
//! doesn't validate, we abort the connection. Period.
//!
//! For self-hosted relay servers using self-signed certs, users explicitly
//! pin the relay's pubkey hash via the signal server's RegisterAck message.

use anyhow::{Context, Result};
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;

/// Production-grade rustls ClientConfig.
/// Uses webpki-roots (Mozilla CA bundle) for the trust anchor set.
/// Will refuse any connection with an invalid cert.
pub fn strict_client_config() -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

/// Strict client config with an additional pinned certificate.
/// For self-hosted relays whose cert is self-signed but whose
/// fingerprint was delivered via a trusted channel (signal server).
pub fn pinned_client_config(pinned_cert_der: Vec<u8>) -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    // Convert the pinned cert and add to trust set.
    let cert = rustls::pki_types::CertificateDer::from(pinned_cert_der);
    roots.add(cert)
        .context("failed to add pinned cert (malformed?)")?;

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

/// IMPORTANT: there is NO function here to disable cert validation.
/// If you find yourself wanting one, the answer is: pin the cert instead.
/// See `pinned_client_config` above.
///
/// If the user passes `MIRU_DANGEROUS_INSECURE_TLS=1`, we LOG a warning but
/// still refuse — they need a code change (and review) to actually disable.
pub fn assert_secure_mode() {
    if std::env::var("MIRU_DANGEROUS_INSECURE_TLS").is_ok() {
        tracing::error!(
            "MIRU_DANGEROUS_INSECURE_TLS is set, but this build does not honor it. \
             Pin a self-signed cert via pinned_client_config() instead."
        );
    }
}
