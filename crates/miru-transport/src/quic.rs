//! QUIC P2P transport via Quinn.
//!
//! Connection strategy:
//!   1. Both peers send/receive hole-punch packets via rendezvous server (addr exchange)
//!   2. Quinn QUIC connects peer-to-peer (UDP, survives most NATs)
//!   3. Media stream = bidirectional QUIC stream (video on stream 0, input on stream 1)
//!   4. Keepalive = QUIC PING frames (15s interval)

use anyhow::{bail, Context, Result};
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, ServerConfig, TransportConfig,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tracing::info;

use miru_common::{crypto::SessionCipher, message::Msg};

/// QUIC stream IDs used by Miru. Each direction of a session type gets
/// its own unidirectional QUIC stream, identified by these IDs.
/// Unused until the QUIC data path is wired in (v1.0).
#[allow(dead_code)]
pub const STREAM_VIDEO: u64 = 0;
#[allow(dead_code)]
pub const STREAM_AUDIO: u64 = 1;
#[allow(dead_code)]
pub const STREAM_INPUT: u64 = 2;
#[allow(dead_code)]
pub const STREAM_CONTROL: u64 = 3;

pub struct QuicTransport {
    conn: Connection,
    /// For outgoing streams' encryption.
    tx_cipher: Arc<SessionCipher>,
    /// For incoming streams' decryption.
    rx_cipher: Arc<SessionCipher>,
    /// RTT tracking — updated on pong; exposed to callers in v1.0.
    #[allow(dead_code)]
    rtt_ms: Arc<std::sync::atomic::AtomicU32>,
}

impl QuicTransport {
    /// Connect as viewer (client) to a host's QUIC endpoint.
    pub async fn connect(
        peer_addr: SocketAddr,
        tx_cipher: SessionCipher,
        rx_cipher: SessionCipher,
    ) -> Result<Self> {
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;
        endpoint.set_default_client_config(insecure_client_config());

        info!("QUIC connect → {}", peer_addr);
        let conn = endpoint
            .connect(peer_addr, "miru")?
            .await
            .context("QUIC handshake failed")?;

        info!("QUIC connected: RTT={}ms", conn.rtt().as_millis());
        Ok(Self::from_conn(conn, tx_cipher, rx_cipher))
    }

    /// Accept as host (server) from incoming QUIC connection.
    pub async fn accept(
        bind_addr: SocketAddr,
        tx_cipher: SessionCipher,
        rx_cipher: SessionCipher,
    ) -> Result<Self> {
        let (server_cfg, _cert) = self_signed_server_config()?;
        let endpoint = Endpoint::server(server_cfg, bind_addr)?;
        info!("QUIC listening on {}", bind_addr);

        let incoming = endpoint.accept().await.context("no incoming connection")?;
        let conn = incoming.await.context("QUIC accept failed")?;
        info!("QUIC accepted from {}", conn.remote_address());
        Ok(Self::from_conn(conn, tx_cipher, rx_cipher))
    }

    fn from_conn(conn: Connection, tx_cipher: SessionCipher, rx_cipher: SessionCipher) -> Self {
        let rtt_ms = Arc::new(std::sync::atomic::AtomicU32::new(
            conn.rtt().as_millis() as u32
        ));
        Self {
            conn,
            tx_cipher: Arc::new(tx_cipher),
            rx_cipher: Arc::new(rx_cipher),
            rtt_ms,
        }
    }

    /// Open a unidirectional send stream for media data (fire-and-forget frames).
    pub async fn open_send_stream(&self) -> Result<QuicSendStream> {
        let stream = self.conn.open_uni().await?;
        Ok(QuicSendStream {
            stream,
            cipher: self.tx_cipher.clone(),
        })
    }

    /// Accept an incoming unidirectional receive stream.
    pub async fn accept_recv_stream(&self) -> Result<QuicRecvStream> {
        let stream = self.conn.accept_uni().await?;
        Ok(QuicRecvStream {
            stream,
            cipher: self.rx_cipher.clone(),
        })
    }

    /// Bidirectional stream for control messages (request/response).
    pub async fn open_bidi(&self) -> Result<(QuicSendStream, QuicRecvStream)> {
        let (tx, rx) = self.conn.open_bi().await?;
        Ok((
            QuicSendStream {
                stream: tx,
                cipher: self.tx_cipher.clone(),
            },
            QuicRecvStream {
                stream: rx,
                cipher: self.rx_cipher.clone(),
            },
        ))
    }

    pub fn rtt_ms(&self) -> u32 {
        self.conn.rtt().as_millis() as u32
    }

    pub fn is_closed(&self) -> bool {
        self.conn.close_reason().is_some()
    }

    pub fn close(&self) {
        self.conn.close(0u32.into(), b"bye");
    }
}

// ─── Stream wrappers ──────────────────────────────────────────────────────────

pub struct QuicSendStream {
    stream: SendStream,
    cipher: Arc<SessionCipher>,
}

impl QuicSendStream {
    /// Send an encrypted message.
    /// Frame format: [4-byte LE length][encrypted payload]
    pub async fn send_msg(&mut self, msg: &Msg) -> Result<()> {
        let json = serde_json::to_vec(msg)?;
        let ct = self.cipher.encrypt(&json)?;
        let len = ct.len() as u32;
        self.stream.write_all(&len.to_le_bytes()).await?;
        self.stream.write_all(&ct).await?;
        Ok(())
    }

    /// Send raw encrypted bytes (for video/audio frames — pre-serialized).
    pub async fn send_raw(&mut self, data: &[u8]) -> Result<()> {
        let ct = self.cipher.encrypt(data)?;
        let len = ct.len() as u32;
        self.stream.write_all(&len.to_le_bytes()).await?;
        self.stream.write_all(&ct).await?;
        Ok(())
    }
}

pub struct QuicRecvStream {
    stream: RecvStream,
    cipher: Arc<SessionCipher>,
}

impl QuicRecvStream {
    /// Receive and decrypt one message.
    pub async fn recv_msg(&mut self) -> Result<Option<Msg>> {
        let mut len_buf = [0u8; 4];
        if self.stream.read_exact(&mut len_buf).await.is_err() {
            return Ok(None);
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 64 * 1024 * 1024 {
            bail!("invalid frame length: {len}");
        }
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;
        let plain = self.cipher.decrypt(&buf)?;
        let msg = serde_json::from_slice(&plain)?;
        Ok(Some(msg))
    }

    /// Receive raw bytes.
    pub async fn recv_raw(&mut self) -> Result<Option<Vec<u8>>> {
        let mut len_buf = [0u8; 4];
        if self.stream.read_exact(&mut len_buf).await.is_err() {
            return Ok(None);
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > 64 * 1024 * 1024 {
            bail!("invalid frame length: {len}");
        }
        let mut buf = vec![0u8; len];
        self.stream.read_exact(&mut buf).await?;
        let plain = self.cipher.decrypt(&buf)?;
        Ok(Some(plain))
    }
}

// ─── TLS config (self-signed for P2P) ────────────────────────────────────────

fn insecure_client_config() -> ClientConfig {
    // P2P: identity verified via X25519 app-layer handshake, not TLS cert.
    // The QUIC TLS layer is a transport-level requirement of the QUIC protocol;
    // actual authentication is done at the application layer via Ed25519
    // signatures on the ephemeral keys. Therefore we skip server cert
    // verification here. See docs/CRYPTO.md §2 for the threat model.
    let crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipVerify))
        .with_no_client_auth();

    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(15)));
    transport.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));

    let mut cfg = ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap(),
    ));
    cfg.transport_config(Arc::new(transport));
    cfg
}

fn self_signed_server_config() -> Result<(ServerConfig, Vec<u8>)> {
    let subject_alt_names = vec!["miru".to_string()];
    let cert_params = rcgen::CertificateParams::new(subject_alt_names)?;
    let key_pair = rcgen::KeyPair::generate()?;
    let cert = cert_params.self_signed(&key_pair)?;
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();

    let server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(cert_der.clone())],
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                key_der,
            )),
        )?;

    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(15)));

    let mut cfg = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)?,
    ));
    cfg.transport_config(Arc::new(transport));
    Ok((cfg, cert_der))
}

/// Skip TLS certificate verification — identity verified at app layer.
/// This is safe because Miru's E2E security is handled by the X25519 +
/// Ed25519 handshake at the application protocol layer (see handshake.rs).
#[derive(Debug)]
struct SkipVerify;

impl rustls::client::danger::ServerCertVerifier for SkipVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
