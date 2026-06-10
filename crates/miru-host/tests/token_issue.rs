//! Verifies that a token minted by `miru-host token issue` actually verifies
//! against the host's identity — the exact check the daemon performs when an
//! AI agent connects. Guards the end-to-end "user can mint a usable token" path.

use std::process::Command;

#[test]
fn issued_token_verifies_against_host_identity() {
    use miru_agent::{AgentToken, Capability};
    use miru_auth::DeviceIdentity;

    // Use an isolated config dir so we don't touch the real one.
    let dir = std::env::temp_dir().join(format!("miru-tok-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Run the CLI with XDG_CONFIG_HOME pointed at our temp dir so the host
    // creates its identity there.
    let out = Command::new(env!("CARGO_BIN_EXE_miru-host"))
        .args([
            "token",
            "issue",
            "--cap",
            "screen_read,pointer_move",
            "--ttl-hours",
            "1",
        ])
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .expect("run miru-host token issue");
    assert!(
        out.status.success(),
        "token issue failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let token_str = String::from_utf8(out.stdout).unwrap();
    let token_str = token_str.trim();
    assert!(
        token_str.starts_with("miru-agent."),
        "unexpected token format: {token_str}"
    );

    // Load the SAME identity the CLI used (XDG_CONFIG_HOME/miru/identity).
    let identity = DeviceIdentity::load_or_create(&dir.join("miru").join("identity")).unwrap();

    // Verify exactly as the daemon does.
    let token = AgentToken::parse_and_verify(token_str, &identity.verifying_key)
        .expect("issued token must verify against host identity");

    assert!(token.has_capability(Capability::ScreenRead));
    assert!(token.has_capability(Capability::PointerMove));
    assert!(
        !token.has_capability(Capability::ShellExec),
        "must not grant un-requested cap"
    );
    assert!(token.seconds_remaining() > 0);
}

#[test]
fn default_token_excludes_dangerous_caps() {
    use miru_agent::{AgentToken, Capability};
    use miru_auth::DeviceIdentity;

    let dir = std::env::temp_dir().join(format!("miru-tok-def-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_miru-host"))
        .args(["token", "issue"]) // no --cap → assistant default
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .expect("run token issue");
    assert!(out.status.success());

    let token_str = String::from_utf8(out.stdout).unwrap();
    let identity = DeviceIdentity::load_or_create(&dir.join("miru").join("identity")).unwrap();
    let token = AgentToken::parse_and_verify(token_str.trim(), &identity.verifying_key).unwrap();

    // Safe default must include screen read but exclude shell/file-write.
    assert!(token.has_capability(Capability::ScreenRead));
    assert!(!token.has_capability(Capability::ShellExec));
    assert!(!token.has_capability(Capability::FileWrite));
}
