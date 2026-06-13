# ADR 0017 — P2P Gap: Signal Server Hardcoded `relay:true` + QUIC Uses JSON

**Date:** 2026-06-13  
**Status:** Partially Fixed  
**Deciders:** Claude Socratic Review

---

## Context

Miru's README and CLAUDE.md both say "P2P-first". Three independent bugs
prevented P2P from ever working:

### Bug 1 — Signal server hardcoded `relay: true`

```rust
// signal/src/main.rs — BEFORE
Msg::ConnectAck(ConnectAck {
    relay: true,  // ALWAYS — ignores whether host has a public address
    ..
})
```

Even when the host registered with a STUN-discovered public address, the
`ConnectAck` told the viewer to use the relay.

### Bug 2 — `SignalEvent::ConnectAck` dropped `host_port`

`signaling.rs` mapped `ack.host_addr → relay_addr` (confusing rename) but
omitted `ack.host_port` entirely. The viewer could see the host's IP but not
the port, making a QUIC connection impossible even if it tried.

### Bug 3 — QUIC/Connection used `serde_json` instead of MessagePack

`QuicSendStream::send_msg()` and `Connection::send()` called
`serde_json::to_vec(msg)`. The relay path (`relay.rs`) was already fixed to
use `rmp_serde::to_vec_named`. If QUIC P2P had been wired in, the host and
viewer would have spoken different wire formats across transport boundaries.

---

## Fixes Applied (this PR)

1. **Signal server**: `relay: true` → `relay: host_pub_addr.is_none()`.
   The viewer is now told P2P *may* be possible when the host has a public
   address. The relay token is still issued (relay is always the fallback).

2. **`SignalEvent::ConnectAck`**: renamed `relay_addr` → `host_addr`, added
   `host_port: Option<u16>`. Viewer now sees the complete socket address.

3. **Viewer `session.rs`**: updated to match the new field names; constructs
   `"addr:port"` for the UI and logs whether P2P looks viable.

4. **`quic.rs`**: `serde_json` → `rmp_serde` for `send_msg` and `recv_msg`.

5. **`connection.rs`**: same MessagePack fix for the unified connection type.

---

## What Still Needs to Be Done (v1.0)

The viewer still always connects via relay (`session.rs` step 4). Completing
P2P requires:

1. **Viewer STUN lookup**: call `nat::discover_public_addr()` before
   `signal.request_connect()`, pass real viewer IP:port instead of `"0.0.0.0"`.

2. **Viewer P2P attempt**: after `ConnectAck` with `relay: false`, try
   `QuicTransport::connect(host_pub_addr, ...)` with a 3–5 second timeout.
   Fall through to relay on failure.

3. **Host QUIC listener**: host needs to bind a QUIC endpoint and wait for
   QUIC alongside the relay connection.

4. **NAT type detection**: call `nat::detect_nat_type()` on both sides;
   only attempt hole-punch if both are `FullCone` or `Restricted`.

5. **Hole punch coordination**: use `nat::punch_to_peer()` before the QUIC
   handshake when both sides are behind NAT.

---

## Consequences

- Signal server now correctly describes whether relay is mandatory.
- QUIC/Connection code is consistent with relay wire format.
- The field rename (`relay_addr` → `host_addr`) is a breaking API change
  within the `SignalEvent` enum, but `SignalClient` is not a public API.
- Future P2P implementation can now call the correct `host_addr:host_port`
  without re-parsing a combined string.
