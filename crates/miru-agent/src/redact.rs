//! Audit log redaction matrix.
//!
//! Per the security playbook (THREAT_MODEL §6: privacy and data minimisation),
//! certain action arguments must NEVER be written verbatim to the audit log.
//! Examples:
//!
//! | Capability        | Plaintext field | Redaction policy                    |
//! |-------------------|-----------------|-------------------------------------|
//! | KeyType           | text            | ALWAYS length + SHA-256 (any size)  |
//! | ClipboardWrite    | text            | length + SHA-256 + first-byte tag   |
//! | ClipboardRead     | data / text     | length + SHA-256 + first-byte tag   |
//! | FileRead          | path            | OK to log path, never content       |
//! | FileWrite         | path + bytes    | OK path, never content              |
//! | ShellExec         | argv            | OK, never stdout/stderr             |
//! | OpenUrl           | url             | log only origin (scheme+host+port)  |
//! | ScreenRead        | image           | resolution + region + SHA-256       |
//! | PointerMove/Click | coords + window | OK                                  |
//!
//! This module provides a single entry point — [`redact_action`] — that
//! takes a raw [`serde_json::Value`] payload and returns a privacy-safe one
//! suitable for logging. It must be called BEFORE [`AuditLog::append`].
//!
//! ## Design rule
//!
//! - The redacted form must still be useful for forensics: lengths, hashes,
//!   and structural metadata are preserved so an investigator can prove
//!   "the agent typed exactly this 47-character string" without leaking
//!   what it was.
//! - Hashes are SHA-256, hex-encoded, 64 chars. Truncating to 16 chars
//!   would still be safe but reduces collision resistance below recommended;
//!   we keep full length.
//! - First-byte tags for clipboard let us distinguish ASCII text from binary
//!   (e.g. an image) without revealing contents.

use ring::digest;
use serde_json::{json, Value};

use crate::token::Capability;


/// Redact a raw action payload so it is safe to write to the audit log.
///
/// Keys-of-interest per capability:
///
/// | Capability      | Field redacted | Replaced with             |
/// |-----------------|----------------|---------------------------|
/// | KeyType         | `text`         | `len`, `sha256`, `tag`    |
/// | ClipboardWrite  | `text`         | `len`, `sha256`, `tag`    |
/// | ClipboardRead   | `data`, `text` | `len`, `sha256`, `tag`    |
/// | OpenUrl         | `url`          | `origin`                  |
/// | FileRead        | `bytes`        | `len`, `sha256`           |
/// | FileWrite       | `bytes`        | `len`, `sha256`           |
/// | ScreenRead      | `image`        | `width`, `height`, `sha256` |
///
/// All other keys pass through unchanged.
pub fn redact_action(cap: Capability, mut action: Value) -> Value {
    let obj = match action.as_object_mut() {
        Some(o) => o,
        None => return action, // not an object — nothing to redact
    };

    match cap {
        Capability::KeyType => {
            // Typed text is ALWAYS redacted regardless of length: a 7-char
            // payload is exactly as likely to be a password as a 70-char one.
            // Key names belong in a separate `key` field, which passes through.
            redact_string_field_tagged(obj, "text");
        }
        Capability::ClipboardWrite => {
            redact_string_field_tagged(obj, "text");
        }
        Capability::ClipboardRead => {
            // Defense-in-depth: if a call site logs the clipboard response
            // through this path (instead of redacting at the outbound
            // boundary), the content must still never reach the log.
            redact_string_field_tagged(obj, "data");
            redact_string_field_tagged(obj, "text");
        }
        Capability::OpenUrl => {
            redact_url_to_origin(obj, "url");
        }
        Capability::FileRead | Capability::FileWrite => {
            redact_bytes_field(obj, "bytes");
            redact_bytes_field(obj, "data");
        }
        Capability::ScreenRead => {
            redact_image_field(obj, "image");
            redact_image_field(obj, "data");
        }
        // Pointer / shell / other capabilities have no plaintext-bearing args.
        // ShellExec arguments are deliberately preserved (argv is auditable;
        // stdout/stderr must NOT be appended, which is enforced at the caller).
        _ => {}
    }

    action
}

/// Replace `field`'s string value with `{len, sha256, tag}` where `tag` is
/// the first non-whitespace byte category — useful to distinguish ASCII text
/// from binary blobs (e.g. base64-encoded images on the clipboard).
fn redact_string_field_tagged(obj: &mut serde_json::Map<String, Value>, field: &str) {
    if let Some(Value::String(s)) = obj.get(field) {
        let bytes = s.as_bytes();
        let h = sha256_hex(bytes);
        let len = bytes.len();
        let tag = first_byte_tag(bytes);
        obj.remove(field);
        obj.insert(format!("{field}_len"), json!(len));
        obj.insert(format!("{field}_sha256"), json!(h));
        obj.insert(format!("{field}_tag"), json!(tag));
    }
}

/// Replace a `bytes` array (or base64 string) with length + hash.
fn redact_bytes_field(obj: &mut serde_json::Map<String, Value>, field: &str) {
    let value = match obj.get(field) {
        Some(v) => v,
        None => return,
    };

    let (len, hash) = match value {
        Value::String(s) => {
            // Treat as opaque: hash the string bytes
            let b = s.as_bytes();
            (b.len(), sha256_hex(b))
        }
        Value::Array(arr) => {
            // Array of u8-sized numbers
            let bytes: Vec<u8> = arr.iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            (bytes.len(), sha256_hex(&bytes))
        }
        _ => return,
    };

    obj.remove(field);
    obj.insert(format!("{field}_len"), json!(len));
    obj.insert(format!("{field}_sha256"), json!(hash));
}

/// Replace a base64 image / pixel buffer with size + hash.
fn redact_image_field(obj: &mut serde_json::Map<String, Value>, field: &str) {
    let value = match obj.get(field) {
        Some(v) => v,
        None => return,
    };

    let (len, hash) = match value {
        Value::String(s) => (s.len(), sha256_hex(s.as_bytes())),
        Value::Array(arr) => {
            let bytes: Vec<u8> = arr.iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            (bytes.len(), sha256_hex(&bytes))
        }
        _ => return,
    };

    obj.remove(field);
    obj.insert(format!("{field}_len"), json!(len));
    obj.insert(format!("{field}_sha256"), json!(hash));
}

/// Replace a full URL with `<scheme>://<host>[:<port>]` so we record what
/// origin was contacted, never the full path / query / fragment which may
/// contain auth tokens, session IDs, or PII.
fn redact_url_to_origin(obj: &mut serde_json::Map<String, Value>, field: &str) {
    let url = match obj.get(field).and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return,
    };
    let origin = parse_origin(&url).unwrap_or_else(|| "<unparseable>".into());
    obj.insert(field.to_string(), json!(origin));
}

/// Parse a URL into its origin (scheme://host[:port]) without pulling in
/// a heavyweight URL crate. Returns None on malformed input.
fn parse_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || scheme.len() > 16 { return None; }
    if !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+') {
        return None;
    }
    // Trim everything after first '/', '?' or '#' to drop path/query/fragment.
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() { return None; }
    Some(format!("{scheme}://{host}"))
}

/// First-byte category tag for tagged redaction.
fn first_byte_tag(bytes: &[u8]) -> &'static str {
    match bytes.iter().find(|&&b| !b.is_ascii_whitespace()) {
        Some(b) if *b == 0x89 || *b == 0x47 || *b == 0xFF => "binary_image", // PNG / GIF / JPEG
        Some(b) if b.is_ascii_alphanumeric() || *b == b'<' || *b == b'/' => "text",
        Some(b) if *b == b'{' || *b == b'[' => "json_or_array",
        Some(_) => "binary",
        None => "empty",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let d = digest::digest(&digest::SHA256, bytes);
    let mut s = String::with_capacity(64);
    for b in d.as_ref() {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Capability;

    #[test]
    fn keytype_short_secret_redacted() {
        // Short typed text is just as likely to be a password ("hunter2")
        // as a key name — it must NEVER reach the log verbatim.
        let action = json!({"text": "hunter2"});
        let r = redact_action(Capability::KeyType, action);
        assert!(r["text"].is_null());
        assert_eq!(r["text_len"], 7);
        assert_eq!(r["text_sha256"].as_str().unwrap().len(), 64);
        let ser = serde_json::to_string(&r).unwrap();
        assert!(!ser.contains("hunter2"));
    }

    #[test]
    fn clipboard_read_response_redacted() {
        // Defense-in-depth: a clipboard response logged through redact_action
        // must not leak its content.
        let action = json!({"data": "api-key sk-123456789", "format": "text"});
        let r = redact_action(Capability::ClipboardRead, action);
        assert!(r["data"].is_null());
        assert_eq!(r["format"], "text");
        let ser = serde_json::to_string(&r).unwrap();
        assert!(!ser.contains("sk-123456789"));
    }

    #[test]
    fn keytype_long_redacted() {
        let secret = "x".repeat(200);
        let action = json!({"text": &secret});
        let r = redact_action(Capability::KeyType, action);
        assert!(r["text"].is_null());
        assert_eq!(r["text_len"], 200);
        assert!(r["text_sha256"].as_str().unwrap().len() == 64);
        // The plaintext must NOT appear anywhere in the serialised form.
        let ser = serde_json::to_string(&r).unwrap();
        assert!(!ser.contains(&secret));
    }

    #[test]
    fn clipboard_write_always_redacted() {
        let action = json!({"text": "credit-card 4242424242424242"});
        let r = redact_action(Capability::ClipboardWrite, action);
        assert!(r["text"].is_null());
        assert_eq!(r["text_tag"], "text");
        assert!(r["text_sha256"].is_string());
        assert!(r["text_len"].as_u64().unwrap() > 0);
    }

    #[test]
    fn open_url_origin_only() {
        let action = json!({"url": "https://example.com/secret-path?token=abc123#section"});
        let r = redact_action(Capability::OpenUrl, action);
        assert_eq!(r["url"], "https://example.com");
    }

    #[test]
    fn open_url_handles_malformed() {
        let action = json!({"url": "not-a-url"});
        let r = redact_action(Capability::OpenUrl, action);
        assert_eq!(r["url"], "<unparseable>");
    }

    #[test]
    fn pointer_move_passes_through() {
        let action = json!({"x": 100, "y": 200, "display": 0});
        let r = redact_action(Capability::PointerMove, action.clone());
        assert_eq!(r, action);
    }

    #[test]
    fn screen_read_image_redacted() {
        let big_image: Vec<u8> = vec![0xFFu8; 1024];
        let action = json!({"width": 800, "height": 600, "image": big_image});
        let r = redact_action(Capability::ScreenRead, action);
        assert_eq!(r["width"], 800);
        assert!(r["image"].is_null());
        assert_eq!(r["image_len"], 1024);
        assert!(r["image_sha256"].is_string());
    }

    #[test]
    fn shell_exec_argv_preserved() {
        let action = json!({"cmd": "ls", "args": ["-la", "/tmp"]});
        let r = redact_action(Capability::ShellExec, action.clone());
        // argv is auditable; the call site enforces no stdout logging.
        assert_eq!(r, action);
    }

    #[test]
    fn redaction_is_deterministic() {
        let secret = "hunter2".repeat(20);
        let a1 = redact_action(Capability::KeyType, json!({"text": &secret}));
        let a2 = redact_action(Capability::KeyType, json!({"text": &secret}));
        assert_eq!(a1, a2, "redaction must be deterministic for replay verification");
    }

    #[test]
    fn redaction_distinguishes_different_secrets() {
        let a1 = redact_action(Capability::KeyType, json!({"text": "a".repeat(200)}));
        let a2 = redact_action(Capability::KeyType, json!({"text": "b".repeat(200)}));
        assert_ne!(a1["text_sha256"], a2["text_sha256"]);
    }

    #[test]
    fn clipboard_text_tagged() {
        let action = json!({"text": "hello world"});
        let r = redact_action(Capability::ClipboardWrite, action);
        assert_eq!(r["text_tag"], "text");
    }

    #[test]
    fn clipboard_json_tagged() {
        let action = json!({"text": "{\"foo\": 1}"});
        let r = redact_action(Capability::ClipboardWrite, action);
        assert_eq!(r["text_tag"], "json_or_array");
    }
}
