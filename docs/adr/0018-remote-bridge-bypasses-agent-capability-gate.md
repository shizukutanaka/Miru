# ADR 0018 — RemoteBridge Connects as Viewer, Bypassing Agent Capability Gate

**Date:** 2026-06-13  
**Status:** Accepted (Design Gap — Tracked)  
**Deciders:** Claude Socratic Review

---

## Context

`miru-mcp`'s `RemoteBridge` connects to a Miru host over the relay and
performs MCP tool calls on behalf of an AI agent. It connects using
`viewer_handshake` with `role: Role::Viewer`.

The host's session loop checks `result.peer_role == Role::AiAgent` to
decide whether to apply the `AgentHandler` capability gate. Because
`RemoteBridge` sends `Role::Viewer`, the host treats it as a human viewer:

| Property                      | Human Viewer | AiAgent (correct) | RemoteBridge (actual) |
|-------------------------------|:---:|:---:|:---:|
| Ed25519 handshake             | ✓   | ✓   | ✓ |
| ACL / TOFU auth               | ✓   | ✓   | ✓ |
| Capability token verification | –   | ✓   | ✗ (bypassed) |
| Per-action audit log          | –   | ✓   | ✗ (bypassed) |
| Dangerous-cap denial          | –   | ✓   | ✗ (bypassed) |
| Rate limiting at host layer   | –   | ✓   | ✗ (bypassed, MCP-layer only) |

The MCP server does apply its own `RateLimiter` before sending inputs, so
burst-rate attacks are throttled. But capability scope restrictions and
the tamper-evident audit log are fully skipped.

---

## Root Cause

`RemoteBridge::connect()` calls `viewer_handshake`, which hardcodes
`role: Role::Viewer` in the Hello message. There is no code path in
`remote_bridge.rs` to send an agent token or to use `Role::AiAgent`.

The `AgentToken` / `AgentSession` infrastructure exists and works, but was
not wired to `RemoteBridge` in v0.1.

---

## Decision

Document this as a known v0.1 limitation. The fix for v0.3 is:

1. Issue an agent token via the host UI (or `HostConfig`) before the MCP
   session starts. Write the token string to `MIRU_AGENT_TOKEN` env var.

2. `RemoteBridge::connect()` reads `MIRU_AGENT_TOKEN`; if present, it:
   - Uses `Role::AiAgent` in the Hello message.
   - Appends the token as the 4th segment of the pubkey field:
     `<eph>:<identity>:<sig>:<miru-agent.payload.sig>`

3. The host's session loop verifies the token against `config.identity.signing_key`
   and enforces capability restrictions + audit logging via `AgentHandler`.

---

## Consequences

**Until v0.3:** MCP server actions (mouse, keyboard, clipboard, URL) are
gated only by the MCP-layer `RateLimiter`. A compromised MCP process
connecting through a valid Miru relay session can exfiltrate clipboard
contents, open arbitrary URLs, and inject arbitrary keystrokes, with no
audit trail on the host side.

**Risk mitigation:** The relay token system means only authenticated
callers who know the target device_id can even reach a host. The
TOFU fingerprint is shown in logs. The MCP rate limiter prevents
rapid automated attacks.

**After v0.3:** Every agent action is capability-scoped, logged with a
hash-chained audit trail, and dangerous capabilities (ShellExec, FileWrite,
ClipboardRead) are denied headlessly.
