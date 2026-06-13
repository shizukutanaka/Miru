# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Developer notes** (sprint log) moved to [`docs/DEVELOPMENT_LOG.md`](docs/DEVELOPMENT_LOG.md).

---

## [Unreleased]

### Added
- XRandR 1.5 multi-monitor enumeration in `miru-capture` (Linux): `displays()` now
  returns per-CRTC `DisplayInfo` with real names and dimensions via
  `randr::get_monitors`; `next_frame()` captures only the selected monitor's region
  using `(x, y, w, h)` offset in `shm::get_image` (#x11-multimonitor)
- Integration tests for X11 capture (`crates/miru-capture/tests/x11_capture.rs`):
  display enumeration, frame dimension validation, out-of-range rejection — run under
  Xvfb in CI
- Audio codec negotiation in handshake: `negotiate_audio()` selects Opus > PCM from
  the intersection of host/viewer `audio_codecs`; falls back to PCM when either side
  omits the field (backward compatibility with older peers)
- Frame backpressure wiring: `FrameController` shared between capture thread and
  session loop; `should_capture()` skips encoding when viewer falls behind (cheapest
  skip point); `on_send()`/`on_ack()` track in-flight frames via Pong round-trips
- `SessionMetrics` struct now drives the session loop; replaces manual
  `video_frames`/`total_bytes` counters; `snapshot()` builds `SessionMetadata` for
  Rekor anchoring at session end
- Merkle tree RFC 6962 deviation pinned by test (`merkle_odd_leaf_self_hash_is_stable`):
  documents and locks the intentional self-hash strategy for odd leaves
- **mDNS discovery** wired into `miru-host`: announces on LAN via `_miru._tcp.local.`
  at startup; `miru-discovery` crate now uses `String` device_id (matching
  `miru-common::session::DeviceId`) instead of `Uuid`, resolving the type mismatch
- **BBR-style congestion control** replaces AIMD `QosController` in session loop:
  `BbrQos` tracks 10-second rolling min-RTT and max-delivery-rate windows, drives
  bitrate through Startup → Drain → ProbeBW → ProbeRTT phases; fed from Pong RTT
  (µs precision) and per-second delivery rate estimated from frame bytes sent
- **File transfer receive** wired into host session loop for `Permission::Full` peers:
  validates temp paths via `safe_fs::resolve_safe_path()`, sanitizes filenames,
  verifies SHA-256 hash on completion; saves to `MIRU_FILE_TRANSFER_DIR` (default
  `~/Downloads/Miru/`); aborts cleanly delete the temp file
- **Multi-monitor display switching**: host sends `Msg::DisplayList` after cipher
  install; viewer UI shows `DisplayTabs` for hosts with 2+ displays; `SelectDisplay`
  restarts the capture loop on a new capturer (old thread exits when channel closes)
- **Opus audio playback** wired in viewer: `AudioDecoder` + `AudioPlayer` run on a
  dedicated `std::thread` (cpal::Stream is !Send); session loop forwards
  `Msg::AudioFrame` via `sync_channel(16)` non-blocking; lazy-inits on first frame
- `Msg::DisplayList` emitted to "display-list" Tauri event; `select_display(index)`
  Tauri command sends `Msg::SelectDisplay` through session cmd channel
- Viewer UI: host fingerprint shown in overlay (blue monospace, hover tooltip for
  out-of-band verification); QoS row shows host-reported fps/bitrate/quality
- GitHub Actions workflows activated: lint, MSRV, test (ubuntu/windows/macos),
  crypto-gate (unwrap budget ≤ 21, strict Ed25519, no TLS bypass, SessionCipher
  !Clone), signal integration test, coverage

### Fixed
- `available_codecs()` no longer advertises HW codecs (H264/H265/AV1) whose
  `HwEncoderBackend::encode()` stubs bail on the first frame, silently killing
  sessions; add test asserting every advertised codec is constructable
- `StubBridge` in `miru-mcp` now requires explicit `MIRU_ALLOW_STUB=1` env var;
  without it the server exits with a clear error rather than silently returning
  fake 1×1 PNG frames and no-op input
- **Ping/Pong protocol corrected**: viewer was handling `Msg::Pong` (never sent by
  host) instead of `Msg::Ping`; fix echoes `Msg::Pong{ts, server_ts}` so the host's
  BBR RTT measurement works correctly
- **15-second handshake timeout** added to both host and viewer to prevent sessions
  hanging indefinitely on unresponsive peers
- Signal server `relay_port` was hardcoded to `21117` in 3 places despite
  `MIRU_RELAY_PORT` env var; now stored in `AppState.relay_port` and used consistently
- **QoS updates** emitted to viewer UI via "qos-update" Tauri event; viewer handles
  `Msg::QosUpdate` from host BBR controller

---

## [0.1.0] - 2026-06-13

First public release — fully self-hostable P2P remote desktop with AI-agent
integration.

### Added

#### Core protocol
- Protocol v1 fully specified (`docs/protocol.md`): message framing, codec
  negotiation, handshake transcript, replay-protection bitmap
- X25519 ephemeral ECDH + HKDF-SHA256 session key derivation with PFS
- Directional ChaCha20-Poly1305 subkeys (`miru-h2v-v1` / `miru-v2h-v1`) so each
  direction has an independent nonce counter — structural nonce-collision prevention
- 128-bit sliding-window replay protection (highest + bitmap tracking)
- Ed25519 identity with `verify_strict` (ZIP-215): small-order keys and
  non-canonical `R` rejected; CI crypto-gate grep-guards all production call sites
- `SessionCipher` cannot be `Clone`-derived (compile-time nonce-reuse prevention)

#### Screen capture
- Windows: DXGI Desktop Duplication (GPU texture, zero-copy)
- macOS: `CGDisplayCreateImage` (deprecated path; ScreenCaptureKit planned for v0.2)
- Linux: MIT-SHM X11 (`shm::get_image`) with XRandR 1.5 monitor enumeration

#### Codec
- VP9 via libvpx (royalty-free, ~30 fps software encode)
- JPEG fallback (always available, no external dep)
- Codec negotiation priority: AV1 > VP9 > VP8 > H265 > H264 > JPEG
- Audio: Opus > PCM negotiation

#### Transport
- WebSocket relay (tokio-tungstenite) with token-gated relay slots
- STUN + UDP hole-punch for P2P (QUIC planned for v0.2)
- Signal server: rendezvous, NAT traversal, relay fallback (Docker-deployable)
- 60-second TTL reaper for unclaimed relay slots

#### AI agent integration
- `miru-agent`: capability tokens (11 capabilities, 3-tier: Normal/Conditional/Dangerous)
  signed by host's Ed25519 identity key; TTL ≤ 15 min hard cap; JTI revocation store
  with 1 M-entry limit; auto-approve cache FIFO-bounded at 1 000 entries
- Hash-chain audit log with mandatory redaction at `append()`: KeyType/ClipboardRead
  data always redacted (len + SHA-256 + tag), URL origin-only, ScreenRead frame-hash
- `miru-mcp`: stdio MCP server for Claude Desktop / Cursor / Codex integration;
  parent-process allowlist; per-capability rate limit; confirmation gate for
  Dangerous capabilities (headless host logs and denies)
- `miru-host token issue` CLI: mint capability tokens from host identity key,
  default excludes ShellExec/FileWrite

#### Security / supply chain
- `cargo-deny`: license allowlist, advisory deny, source allowlist, sha1 ban
- `cargo-audit` in CI
- Sigstore keyless cosign signing of release binaries (SBOM planned)
- Transparency anchoring: `CoSignedCommitment` + Rekor (optional, non-fatal)
- safe_fs: symlink rejection, `O_NOFOLLOW`, canonicalize + prefix allowlist
  (addresses RustDesk CVE-2026-2490 class)
- sandbox: Linux seccomp/landlock + macOS sandbox-exec per process role
- Token clock validation: `iat ≤ exp`, `MAX_CLOCK_SKEW_SECS = 300`

#### CI / developer experience
- MSRV job (pinned toolchain from `rust-toolchain.toml` + `--locked`)
- Unwrap budget gate: production `unwrap()`/`expect()` count ≤ budget (21);
  new additions fail CI until `docs/UNWRAP_TRIAGE.md` is updated
- Crypto invariant gate: grep blocks non-strict `Ed25519::verify()` in production
- Coverage via `cargo-llvm-cov` → Codecov
- Cross-compilation matrix: x86_64/aarch64 Linux, x86_64 Windows, x86_64/aarch64 macOS
- `miru-host audit verify|show` subcommand for log inspection without daemon

#### Documentation
- `docs/architecture.md`, `docs/protocol.md`, `docs/roadmap.md`
- `docs/THREAT_MODEL.md` (STRIDE-based), `docs/CRYPTO.md` (invariants + transcript)
- `docs/INCIDENT_RESPONSE.md`, `docs/UNWRAP_TRIAGE.md`
- ADR 0001–0012 in `docs/adr/`
- `SECURITY.md`, `CONTRIBUTING.md`

### Fixed
- Toolchain pin 1.80 was unbuildable with `Cargo.lock` (edition2024/rustc 1.88
  required by transitive deps); bumped to 1.88 and added MSRV CI job
- `miru-client` `SignalClient::connect` argument drift (2 → 3 args with identity pubkey)
- `miru-mcp` e2e test missing `rand` dev-dependency
- `miru-codec` `vpx` feature: `_fps`/`_bitrate_kbps` parameter name mismatch
- Rekor TLS failure: `reqwest` had no TLS backend; fixed with `rustls-tls` feature
- Audit log created world-readable for a brief window; fixed with `OpenOptions::mode(0o600)`
- Signal server relay slots leaked on timeout; added 60-second reaper
- `qos_bbr` used `.unwrap()` on deque operations; replaced with safe `if let` patterns
- `KeyType` short secrets (<64 bytes) were logged in plaintext; now always redacted
- Token `iat > exp` not rejected; fixed with explicit validation + test
- Revocation list / auto-approve cache had unbounded growth; capped at 1 M / 1 000 entries

### Security
- Ed25519 ZIP-215 strict verification enforced workspace-wide
- TLS-disabling APIs (`danger_accept_invalid_certs`, `with_no_client_auth`) blocked
  by CI grep gate (per RustDesk CVE-2026-30794 class)
- `SessionCipher` non-clonable prevents AEAD nonce reuse

---

## [0.0.1] - 2026-05-01 (internal prototype)

Initial architecture: protocol skeleton, X25519 handshake stub, libvpx VP9
encode/decode, Tauri viewer UI scaffold, signal server WebSocket skeleton.

---

[Unreleased]: https://github.com/shizukutanaka/miru/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/shizukutanaka/miru/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/shizukutanaka/miru/releases/tag/v0.0.1
