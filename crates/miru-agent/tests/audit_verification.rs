use miru_agent::audit::{AuditLog, AuditOutcome};
use miru_agent::Capability;
use serde_json::json;

#[test]
fn generate_sample_log() {
    let path = std::path::PathBuf::from("/tmp/audittest/agent_audit.log");
    let _ = std::fs::remove_file(&path);
    let log = AuditLog::open(&path).unwrap();
    log.append(
        "jti-1",
        Capability::ScreenRead,
        json!({"kind":"screen_capture_provenance","frame_seq":0,"sha256":"abc"}),
        None,
        AuditOutcome::Ok,
    )
    .unwrap();
    log.append(
        "jti-1",
        Capability::PointerClick,
        json!({"x":10,"y":20}),
        None,
        AuditOutcome::Ok,
    )
    .unwrap();
    log.append(
        "jti-1",
        Capability::ShellExec,
        json!({"cmd":"ls"}),
        Some(false),
        AuditOutcome::Denied,
    )
    .unwrap();
    assert_eq!(log.verify().unwrap(), 3);
}

#[test]
fn verify_path_detects_tamper() {
    use miru_agent::audit::{read_all_verified, verify_path};
    let dir = std::env::temp_dir().join(format!("miru-audit-tamper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.log");
    let _ = std::fs::remove_file(&path);

    {
        let log = AuditLog::open(&path).unwrap();
        log.append(
            "j",
            Capability::ScreenRead,
            json!({"v":1}),
            None,
            AuditOutcome::Ok,
        )
        .unwrap();
        log.append(
            "j",
            Capability::PointerClick,
            json!({"v":2}),
            None,
            AuditOutcome::Ok,
        )
        .unwrap();
        log.append(
            "j",
            Capability::KeyType,
            json!({"v":3}),
            None,
            AuditOutcome::Ok,
        )
        .unwrap();
    }

    // Clean log verifies and reads back all entries.
    assert_eq!(verify_path(&path).unwrap(), 3);
    assert_eq!(read_all_verified(&path).unwrap().len(), 3);

    // Tamper a non-last entry → chain must break.
    let content = std::fs::read_to_string(&path).unwrap();
    let tampered = content.replacen("\"v\":1", "\"v\":999", 1);
    std::fs::write(&path, tampered).unwrap();
    assert!(
        verify_path(&path).is_err(),
        "tamper on entry #0 must be detected"
    );
    assert!(read_all_verified(&path).is_err());
}
