# ADR 0010 — MCP Tool-Call Replay Protection

Status: Accepted
Date: 2026-05-29

## Context

The capability-token system (ADR 0009) binds an AI agent's permissions with an
Ed25519 signature and a short TTL (≤15 min). The per-capability rate limiter
bounds request *frequency*. Neither prevents *replay*: an adversary able to
observe or inject on the stdio channel between the MCP host (Claude Desktop) and
`miru-mcp` could capture a validly-authorized tool call and resend it.

arXiv:2601.17549 ("Breaking the Protocol", Maloyan & Namiot, Jan 2026) analyzes
the MCP specification and finds, among three protocol-level weaknesses, that MCP
v1.0 messages have **no replay protection** (Table III: Replay Protection
"Required: Yes / MCP v1.0: No"). Their AttestMCP extension (§VI-C) adds a
`(timestamp, nonce)` pair per message with a sliding-window guard, reducing
overall attack success from 52.8% to 12.4%.

## Decision

Implement a `ReplayGuard` in `crates/miru-mcp/src/replay.rs` following AttestMCP
§VI-C parameters:

- **Sliding window of 1000 nonces** per server, FIFO eviction.
- **30-second validity window** on the request timestamp (covers clock skew +
  transit in both directions via `abs_diff`).
- A request is `Fresh` only if its timestamp is within the window AND its nonce
  has not been seen; otherwise `DuplicateNonce` or `StaleTimestamp`.

`McpServer::check_replay(timestamp, nonce)` exposes the guard. Calls that carry
replay metadata are checked; v0.1 stdio calls without metadata fall back to the
existing parent-allowlist + token-TTL defenses (the stdio channel is a local
pipe from an allowlisted parent, a much smaller attack surface than a network
socket). Networked or forwarded tool calls must always supply the metadata.

## Why not require metadata on every call now

The MCP stdio JSON-RPC envelope from Claude Desktop does not yet carry an
`mcpsec` block (that is an AttestMCP proposal for MCP v2.0, not shipped). Forcing
it would break compatibility with current hosts. We implement the guard now so
the enforcement path exists and is tested; we enable mandatory metadata when a
host emits it or when calls arrive over the network (v1.0 process-separation
work, where miru-mcp talks to the host over a socket rather than inheriting a
pipe).

## Consequences

+ Replay of a captured tool call is rejected once the nonce is recorded or the
  30s window lapses.
+ Dependency-free (`VecDeque` + `HashSet`), O(1) per check, safe under the
  single-threaded stdio loop and under contention (Mutex-guarded).
+ Tested: 5 unit tests in `replay.rs` (fresh / duplicate / stale / distinct /
  window-eviction) + 3 integration tests through `McpServer` in
  `local_bridge_e2e.rs`.
− Does not by itself authenticate the message (no HMAC yet). Message
  authentication (AttestMCP principle #2) is deferred to v1.0 process
  separation, where a shared secret between host and miru-mcp becomes available;
  until then the local-pipe + parent-allowlist boundary is the trust anchor.

## References

- arXiv:2601.17549 §III-D (Message Integrity Analysis), §VI-C (Protocol Additions)
- ADR 0009 (MCP token design)
- `crates/miru-mcp/src/replay.rs`
