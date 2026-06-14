//! Sigstore Rekor compatibility — public transparency log posting.
//!
//! After a session, both peers agree on a CoSignedCommitment. The host can
//! optionally post the commitment to a Rekor instance for public verifiability.
//!
//! Format used: Rekor's `intoto` v0.0.2 attestation type, which accepts
//! arbitrary JSON predicates. Our predicate type is `https://miru.app/v0/session-commitment`.
//!
//! The cost of posting is one HTTPS request; the benefit is anyone can later
//! prove the commitment existed at a particular point in time without trusting
//! Miru, the host, or the viewer.

use crate::{CoSignedCommitment, HostOnlyAttestation, SessionMetadata};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Default public Rekor instance run by the Sigstore project.
pub const DEFAULT_REKOR_URL: &str = "https://rekor.sigstore.dev";

/// In-toto attestation predicate type for Miru session commitments.
pub const PREDICATE_TYPE: &str = "https://miru.app/v0/session-commitment";

/// Build the in-toto statement that gets posted to Rekor.
pub fn build_statement(
    metadata: &SessionMetadata,
    commitment: &CoSignedCommitment,
) -> serde_json::Value {
    json!({
        "_type": "https://in-toto.io/Statement/v0.1",
        "subject": [{
            "name": format!("miru-session/{}", metadata.session_id),
            "digest": {
                "sha256": commitment.commitment_b64,
            },
        }],
        "predicateType": PREDICATE_TYPE,
        "predicate": {
            "session_id": metadata.session_id,
            "host_pubkey": metadata.host_pubkey_b64,
            "viewer_pubkey": metadata.viewer_pubkey_b64,
            "codec": metadata.codec,
            "started_at": metadata.started_at,
            "ended_at": metadata.ended_at,
            "video_frames": metadata.video_frames,
            "total_bytes": metadata.total_bytes,
            "relayed": metadata.relayed,
            "host_signature": commitment.host_signature_b64,
            "viewer_signature": commitment.viewer_signature_b64,
            "nonce": commitment.nonce_b64,
        },
    })
}

/// Build the in-toto statement for a host-only attestation.
///
/// The predicate includes `"signer": "host_only"` so verifiers know that
/// only the host signature should be checked — there is no viewer signature.
pub fn build_host_statement(
    metadata: &SessionMetadata,
    attestation: &HostOnlyAttestation,
) -> serde_json::Value {
    json!({
        "_type": "https://in-toto.io/Statement/v0.1",
        "subject": [{
            "name": format!("miru-session/{}", metadata.session_id),
            "digest": {
                "sha256": attestation.commitment_b64,
            },
        }],
        "predicateType": PREDICATE_TYPE,
        "predicate": {
            "signer": "host_only",
            "session_id": metadata.session_id,
            "host_pubkey": metadata.host_pubkey_b64,
            "viewer_pubkey": metadata.viewer_pubkey_b64,
            "codec": metadata.codec,
            "started_at": metadata.started_at,
            "ended_at": metadata.ended_at,
            "video_frames": metadata.video_frames,
            "total_bytes": metadata.total_bytes,
            "relayed": metadata.relayed,
            "host_signature": attestation.host_signature_b64,
            "nonce": attestation.nonce_b64,
        },
    })
}

/// Post a host-only session attestation to a Rekor transparency log.
///
/// Non-fatal by design — Rekor unavailability must not prevent session teardown.
pub async fn submit_host_to_rekor(
    rekor_url: &str,
    metadata: &SessionMetadata,
    attestation: &HostOnlyAttestation,
) -> anyhow::Result<RekorEntry> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;

    if !rekor_url.starts_with("https://") {
        anyhow::bail!(
            "Rekor URL must use HTTPS to prevent MITM fake-success injection (got: {rekor_url:?})"
        );
    }

    let statement = build_host_statement(metadata, attestation);
    let statement_b64 = B64.encode(serde_json::to_vec(&statement)?);

    let body = serde_json::json!({
        "kind": "intoto",
        "apiVersion": "0.0.1",
        "spec": {
            "content": {
                "envelope": statement_b64,
                "hash": {
                    "algorithm": "sha256",
                    "value": hex_sha256(serde_json::to_vec(&statement)?.as_slice()),
                }
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("miru-transparency/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let resp = client
        .post(format!(
            "{}/api/v1/log/entries",
            rekor_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Rekor POST failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Rekor returned {}: {}", status, &text[..text.len().min(200)]);
    }

    let map: serde_json::Map<String, serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Rekor response parse: {e}"))?;

    let (uuid, entry_body) = map
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Rekor returned empty map"))?;

    Ok(RekorEntry {
        log_index: entry_body["logIndex"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing logIndex"))?,
        uuid,
        log_id: entry_body["logID"].as_str().unwrap_or("").to_string(),
        integrated_time: entry_body["integratedTime"].as_i64().unwrap_or(0),
    })
}

/// Result of posting to Rekor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RekorEntry {
    /// The log index.
    #[serde(rename = "logIndex")]
    pub log_index: u64,
    /// Sigstore-format UUID of this entry.
    pub uuid: String,
    /// SHA-256 of the entry body (the proof of inclusion will reference this).
    #[serde(rename = "logID")]
    pub log_id: String,
    /// When Rekor accepted the entry (unix seconds).
    #[serde(rename = "integratedTime")]
    pub integrated_time: i64,
}

/// Build the URL where users can later verify this entry.
pub fn entry_url(rekor_url: &str, uuid: &str) -> String {
    format!(
        "{}/api/v1/log/entries/{}",
        rekor_url.trim_end_matches('/'),
        uuid
    )
}

/// Post a session commitment to a Rekor transparency log.
///
/// Returns the `RekorEntry` on success with the log index + UUID for later
/// verification. Non-fatal by design: callers should log the error and continue
/// (Rekor unavailability must not prevent sessions from completing).
///
/// # Example
/// ```no_run
/// use miru_transparency::{rekor, SessionMetadata, CoSignedCommitment};
/// # async fn example(meta: &SessionMetadata, commitment: &CoSignedCommitment) {
/// match rekor::submit_to_rekor(rekor::DEFAULT_REKOR_URL, meta, commitment).await {
///     Ok(entry) => tracing::info!("Anchored in Rekor at index={}", entry.log_index),
///     Err(e)    => tracing::warn!("Rekor unavailable (non-fatal): {}", e),
/// }
/// # }
/// ```
pub async fn submit_to_rekor(
    rekor_url: &str,
    metadata: &crate::SessionMetadata,
    commitment: &crate::CoSignedCommitment,
) -> anyhow::Result<RekorEntry> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;

    if !rekor_url.starts_with("https://") {
        anyhow::bail!(
            "Rekor URL must use HTTPS to prevent MITM fake-success injection (got: {rekor_url:?})"
        );
    }

    let statement = build_statement(metadata, commitment);

    // Rekor's /api/v1/log/entries endpoint accepts a hashedrekord or intoto entry.
    // We use the intoto kind, which directly accepts our in-toto statement.
    let statement_b64 = B64.encode(serde_json::to_vec(&statement)?);

    let body = serde_json::json!({
        "kind": "intoto",
        "apiVersion": "0.0.1",
        "spec": {
            "content": {
                "envelope": statement_b64,
                "hash": {
                    "algorithm": "sha256",
                    "value": hex_sha256(serde_json::to_vec(&statement)?.as_slice()),
                }
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("miru-transparency/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let resp = client
        .post(format!(
            "{}/api/v1/log/entries",
            rekor_url.trim_end_matches('/')
        ))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Rekor POST failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Rekor returned {}: {}",
            status,
            &text[..text.len().min(200)]
        );
    }

    // Rekor returns a map of { uuid: entry_body }. Extract the first (and only) entry.
    let map: serde_json::Map<String, serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Rekor response parse: {e}"))?;

    let (uuid, entry_body) = map
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Rekor returned empty map"))?;

    let log_index = entry_body["logIndex"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("missing logIndex"))?;
    let log_id = entry_body["logID"].as_str().unwrap_or("").to_string();
    let integrated_time = entry_body["integratedTime"].as_i64().unwrap_or(0);

    Ok(RekorEntry {
        log_index,
        uuid,
        log_id,
        integrated_time,
    })
}

fn hex_sha256(data: &[u8]) -> String {
    use ring::digest;
    let d = digest::digest(&digest::SHA256, data);
    d.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    use uuid::Uuid;

    #[test]
    fn statement_contains_required_fields() {
        let host = SigningKey::generate(&mut OsRng);
        let viewer = SigningKey::generate(&mut OsRng);
        let meta = SessionMetadata {
            session_id: Uuid::new_v4(),
            host_pubkey_b64: B64.encode(host.verifying_key().as_bytes()),
            viewer_pubkey_b64: B64.encode(viewer.verifying_key().as_bytes()),
            codec: "vp9".into(),
            started_at: 100,
            ended_at: 200,
            video_frames: 10,
            total_bytes: 1024,
            relayed: false,
        };
        let cosigned = CoSignedCommitment::new(&meta, &host, &viewer).unwrap();

        let stmt = build_statement(&meta, &cosigned);
        // Must have subject and predicate at top level
        assert_eq!(stmt["_type"], "https://in-toto.io/Statement/v0.1");
        assert_eq!(stmt["predicateType"], PREDICATE_TYPE);
        // Subject digest must be the commitment
        assert_eq!(
            stmt["subject"][0]["digest"]["sha256"],
            cosigned.commitment_b64
        );
    }

    #[test]
    fn entry_url_well_formed() {
        let url = entry_url(DEFAULT_REKOR_URL, "abc123");
        assert!(url.starts_with("https://rekor.sigstore.dev/"));
        assert!(url.ends_with("/abc123"));
    }

    #[tokio::test]
    async fn http_url_rejected_submit() {
        let host = SigningKey::generate(&mut OsRng);
        let viewer = SigningKey::generate(&mut OsRng);
        let meta = SessionMetadata {
            session_id: Uuid::new_v4(),
            host_pubkey_b64: B64.encode(host.verifying_key().as_bytes()),
            viewer_pubkey_b64: B64.encode(viewer.verifying_key().as_bytes()),
            codec: "vp9".into(),
            started_at: 100,
            ended_at: 200,
            video_frames: 10,
            total_bytes: 1024,
            relayed: false,
        };
        let cosigned = CoSignedCommitment::new(&meta, &host, &viewer).unwrap();

        let err = submit_to_rekor("http://rekor.example.com", &meta, &cosigned)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("HTTPS"),
            "HTTP URL must be rejected: {err}"
        );
    }

    #[tokio::test]
    async fn http_url_rejected_host_submit() {
        let host = SigningKey::generate(&mut OsRng);
        let viewer = SigningKey::generate(&mut OsRng);
        let meta = SessionMetadata {
            session_id: Uuid::new_v4(),
            host_pubkey_b64: B64.encode(host.verifying_key().as_bytes()),
            viewer_pubkey_b64: B64.encode(viewer.verifying_key().as_bytes()),
            codec: "vp9".into(),
            started_at: 100,
            ended_at: 200,
            video_frames: 10,
            total_bytes: 1024,
            relayed: false,
        };
        use crate::HostOnlyAttestation;
        let att = HostOnlyAttestation::new(&meta, &host).unwrap();

        let err = submit_host_to_rekor("http://rekor.example.com", &meta, &att)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("HTTPS"),
            "HTTP URL must be rejected: {err}"
        );
    }
}
