# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Developer notes** (sprint log) moved to [`docs/DEVELOPMENT_LOG.md`](docs/DEVELOPMENT_LOG.md).

---

## [Unreleased]

### Fixed
- **BBR QoS adaptation was inert** (Socratic: the QoS loop computed new fps/bitrate
  every second but only sent `QosUpdate` to the viewer for display — the host's
  own encoder ran at the initial fixed fps/bitrate for the entire session).
  Fixed by adding `target_fps: AtomicU8` and `target_bitrate_kbps: AtomicU32` to
  `FrameController`, seeded from the initial BBR values. The session loop now calls
  `bp.apply_qos(u.fps, u.bitrate_kbps)` on every BBR tick result. The capture
  thread reads these atomics every frame and calls `encoder.update_bitrate()` when
  the value changes. FPS changes update `frame_interval` in-place. No encoder
  restart needed (avoids keyframe disruption on every adaptation).

### Performance
- **Recording uses binary container instead of per-frame JPEG files**: the old
  approach wrote one `{:08}.jpg` file per frame — 18,000 files per 10-min 30fps
  session — causing inode/metadata overhead of ~144MB and `readdir()` slowdown.
  Now writes `frames.bin` (concatenated JPEG data) + `offsets.bin` (12-byte
  `(offset:u64, size:u32)` index per frame). `get_recording_frame(idx)` seeks
  to `idx*12` in `offsets.bin`, reads offset+size, seeks in `frames.bin` — O(1)
  random access preserved. Both files are kept open for the session duration;
  `write_recording_frame` holds the lock only while appending (~µs per frame).
- **Relay protocol switched from JSON to MessagePack** (ADR 0015): `relay.rs`
  was serializing messages with `serde_json`, which encodes `Vec<u8>` as a JSON
  integer array `[0,255,…]` — ~3× size inflation on every VP9/JPEG/audio frame.
  Switched to `rmp-serde` (MessagePack); binary payloads now use MessagePack's
  native bytes type. Added `#[serde(with = "serde_bytes")]` to `VideoFrame.data`,
  `AudioFrame.data`, `ClipboardSync.data`, and `FileTransfer::Chunk.data`.
  Net effect: ~3× reduction in relay wire traffic for video (e.g. 90KB JSON →
  ~31KB msgpack for a 30KB VP9 P-frame). Verified by `msgpack_is_smaller_than_json_for_video_frames` test.
  No API change — the relay server forwards bytes opaquely; no signal server changes needed.

### Security
- **Fix: TOFU pubkey pinning was a no-op** (ADR 0014): `check_or_pair` in
  `miru-host` was passing `device_id` as `pubkey_b64` to both `AclStore::check`
  and `AclStore::trust`. Because the stored value and the argument were the same
  string, `TrustDecision::PubkeyMismatch` could never fire, making TOFU key
  pinning completely ineffective. Any peer that could observe a trusted device_id
  on the signal server could impersonate that device and obtain full control.
  Fixed by using `B64.encode(pubkey)` — the actual Ed25519 pubkey bytes from
  the verified handshake — as `pubkey_b64` in both store and check. Regression
  test `acl_rejects_device_id_impersonation` added to `miru-auth`.
  **Breaking**: existing `acl.json` files must be deleted and re-paired.

### Changed
- **Viewer frame decode off the main thread** (ADR 0013): the session canvas now
  decodes JPEG frames via `createImageBitmap` (off-thread) instead of an `Image` +
  data-URL (main-thread decode + GC pressure). Decoding is single-flight with
  latest-frame-wins: frames arriving while a decode is in flight replace the
  pending slot, so a slow/constrained CPU drops stale frames instead of building
  an input-latency backlog
- **ADR 0013 — viewer codec topology**: documents why the viewer keeps
  VP9-over-network + JPEG-over-IPC rather than wiring the WebGL2 YUV renderer.
  Feeding raw I420 over the Tauri (JSON) IPC bridge would cost ~124 MB/s at 1080p30
  vs ~8 MB/s for JPEG — a ~15x regression, because the real bottleneck is IPC
  bandwidth, not in-browser decode. `yuv-renderer.ts` is marked frozen; the correct
  way to drop the double-codec is a future WebCodecs `VideoDecoder` path

### Added
- **OpenUrl protocol message**: new `Msg::OpenUrl(OpenUrlRequest)` lets the MCP
  (or future viewer UI) ask the host to open a URL in its default browser; handled
  in Full-permission sessions via the `open` crate; `RemoteBridge.open_url()` now
  sends the message instead of returning an error; only http(s) URLs accepted
- **MCP ClipboardRead auto-approved**: the MCP confirmation policy now auto-approves
  `Capability::ClipboardRead` (non-destructive, requires explicitly minted token);
  previously auto-denied; `miru_clipboard_read` tool description updated accordingly
- **STUN host public address in ConnectAck**: host runs STUN on startup (5 s timeout,
  non-fatal); public addr/port included in `Register` message; signal server stores it
  per-device and populates `ConnectAck.host_addr`/`host_port`; viewer captures it and
  shows "経路: リレー (直接: ip:port)" in the session overlay — sets up future direct path
- **Linux text injection**: `InputKind::Text` now works on Linux by writing to clipboard
  via `xclip` then injecting `Ctrl+V` via uinput; previously a no-op warning stub;
  enables the MCP's `miru_key_type` tool on Linux hosts
- **Viewer-initiated clipboard pull**: new `RequestClipboard` protocol message lets the viewer
  explicitly request the host's current clipboard text; "クリップボード受信" toolbar button sends
  the request and the host responds immediately via `ClipboardSync`; `miru-mcp` remote bridge
  also uses this path so AI agents can read clipboard without waiting for host-push events
- **Viewer QoS mode hint** (`QosHint` protocol message): viewer toolbar cycles through three
  modes — "画質重視" (quality: bitrate-biased), "バランス" (default BBR behaviour), "滑らか"
  (smooth: FPS floor 30, bitrate capped at 70% of bottleneck bandwidth); host
  `BbrQos::apply_hint()` persists the hint and applies it on every BBR tick; quality estimate
  biases ±10 units to match the selected mode
- **miru-mcp remote bridge clipboard** (`read_clipboard` now works): sends `RequestClipboard`
  to the host, waits up to 2 s for `ClipboardSync` response; `recv_loop` captures all
  `ClipboardSync` pushes in `latest_clipboard`; `status` payload includes `has_clipboard`
- **Session elapsed timer in viewer**: session screen tracks elapsed seconds since "connected"
  event; overlay shows `T mm:ss` row when a session is active; hours shown when ≥ 1 h
- **LAN signal URL auto-fill**: viewer ConnectScreen now defaults to `ws://localhost:21115/ws`
  (self-hosted); clicking a LAN peer auto-fills the signal URL as `ws://{peer_addr}:{port}/ws`
  for true zero-config LAN access without manual URL entry
- **Viewer codec advertisement fix**: viewer advertises only decodable codecs via
  `miru_codec::available_codecs()` (VP9/VP8/JPEG with `vpx` feature; JPEG-only otherwise);
  previously advertised AV1/H264/H265 stub decoders that would fail at first frame
- **miru-mcp remote bridge**: `RemoteBridge` connects to a Miru host via signal server + relay,
  maintains a live viewer session, decodes VP9/JPEG frames in a background task, returns PNG
  via `capture_screen`; configured via `MIRU_SIGNAL` + `MIRU_HOST_DEVICE_ID` env vars
- **Session recording with Time Travel playback**: `start_recording` / `stop_recording` Tauri
  commands; decoded JPEG frames saved to `{config_dir}/recordings/{session_id}/frames/`; metadata
  JSON written on stop; "録画" button in session toolbar; `TimelineScrubber` wired to
  `get_recording_frame` for per-frame scrubbing with real frames (not placeholder)
- **LAN peer discovery in viewer connect screen**: viewer starts mDNS browse at launch via
  `miru-discovery`; `discover_lan_peers` Tauri command returns `DiscoveredPeer` snapshot;
  ConnectScreen polls every 3 s and shows "LAN上のデバイス" section with one-click connect
- **Signal server per-IP Connect rate limiting**: 20 requests/minute per IP; exceeded requests
  receive `Error{code: 429}`; implemented via `ConnectInfo<SocketAddr>` extractor +
  `DashMap<IpAddr, ConnectBucket>` with sliding 60-second window
- **JPEG codec fast path**: viewer bypasses decode→re-encode for JPEG-codec frames, forwarding
  host bytes directly to frontend — eliminates double-compression quality loss and ~5 ms CPU
  overhead; VP9/VP8 re-encode quality raised from 70 → 85
- **File transfer from viewer to host**: `send_file` Tauri command accepts file name +
  base64 bytes, sanitizes filename, computes SHA-256 hash via `ring::digest`, sends
  `FileTransfer::Start → Chunk* → Done` sequence in 256 KB chunks; viewer UI adds
  hidden file input + "ファイル送信" toolbar button (100 MB limit, disabled during
  transfer and when not connected); host-side receive path already existed (Full permission)
- **Viewer session stats: bitrate and packet loss**: `bitrate_kbps` computed from bytes
  received per second; `packet_loss_pct` estimated from `VideoFrame.seq` gaps; both
  updated each second alongside fps; overlay shows BW kbps row always and PKT% row when
  loss exceeds 0.5% (highlighted amber)
- **Agent token issuance wired**: `AgentTokenIssue` UI now calls the real
  `issue_agent_token` Tauri command instead of mocking a token; error and loading
  states displayed; TTL field renamed `ttl_mins` (was `ttl_hours`), options changed
  to 5/10/15 min matching the 15-minute hard cap enforced by `miru_agent`
- **Linux uinput absolute mouse positioning**: replaced hardcoded 1920×1080 screen size
  with a resolution-independent 0-65535 coordinate space (same as Windows convention);
  adds `UI_ABS_SETUP` ioctl to configure ABS axis ranges so X11/Wayland can correctly
  map virtual device coordinates to physical screen pixels on any resolution
- **Rendezvous server keepalive**: 30-second server-initiated WebSocket ping detects
  dead connections and triggers immediate registry cleanup rather than waiting for
  OS-level TCP timeout (minutes to hours); handles client Pong response silently
- **Relay token validation**: relay server now rejects connections with unknown tokens
  (those not pre-registered by the rendezvous server); prevents resource exhaustion via
  fabricated tokens; unknown token logs a warning and drops the WebSocket immediately;
  relay_e2e tests updated to use full rendezvous flow + new `relay_rejects_unknown_token` test
- **Capture loop clean exit**: `capture_loop` thread now exits on `TrySendError::Disconnected`
  (receiver dropped = session ended) rather than running forever after session teardown;
  distinguishes backpressure (`Full`) from termination (`Disconnected`)
- **Signal server auto-reconnect** in `miru-host`: `session::run()` now loops on
  disconnect with exponential backoff (5 → 10 → 20 → … → 120 s cap); connect
  failures also retry with backoff rather than exiting; host stays alive through
  transient signal-server outages
- **Bidirectional clipboard sync**: host polls clipboard every second and pushes text
  changes to viewer via `Msg::ClipboardSync`; viewer handles the push and writes to local
  clipboard via `navigator.clipboard.writeText()`; `ClipboardFormat` derives `PartialEq`
- **HTML and image clipboard sync** (viewer → host): `ClipboardFormat::Html` and
  `ClipboardFormat::Image` now forwarded via `miru_input::set_clipboard_raw(data, mime)`;
  Linux uses `xclip -t <mime>` for native MIME clipboard; macOS/Windows fall through to
  text for `text/*` and warn-and-skip for binary formats
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
