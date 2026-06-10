# ADR 0012 — Screen-Capture Provenance for Visual Prompt Injection Traceability

Status: Accepted
Date: 2026-05-30

## Context

A 10-category survey of Miru's domains (P2P remote desktop, computer-use agents,
MCP, transport, capabilities, transparency, sandboxing, codecs, TOFU, Rust
systems) surfaced the most acute under-addressed threat from the AI-agent
category: **Visual Prompt Injection (VPI)**.

- VPI-Bench (Cao et al., arXiv:2506.02456) shows Computer-Use Agents are
  deceived at rates up to 51% (and Browser-Use Agents up to 100%) by malicious
  instructions *visually embedded* in rendered interfaces. System-prompt
  defenses give "only limited improvements."
- "The Attacker Moves Second" (Nasr, Carlini et al., arXiv:2510 — 14 authors
  from OpenAI/Anthropic/DeepMind) bypasses 12 published prompt-injection
  defenses with >90% success using adaptive attacks. The lesson: **model-level
  defenses are not a reliable trust boundary.**

Miru's capability gate controls *whether* an agent may read the screen, but once
`ScreenRead` is granted, whatever instructions are embedded in those pixels
reach the model unfiltered. We cannot reliably detect VPI in the pixels (the
literature shows detectors are evadable), and trying to would add a heavy VLM to
a zero-dependency Rust codebase.

## Decision

Do not attempt to *detect* VPI. Instead make every observed frame
**forensically traceable**, turning an undetectable-in-the-moment attack into an
after-the-fact auditable one. On each `miru_screen_capture`:

1. Compute SHA-256 of the exact PNG bytes sent to the agent.
2. Assign a monotonic per-session `frame_seq`.
3. Append a `screen_capture_provenance` record to the tamper-evident audit log:
   `{display, frame_seq, sha256, ts_secs, bytes}`.
4. Echo `frame#N sha256:…` in the tool response text.

Because the audit log is hash-chained (ADR: agent audit) and periodically
anchored to Rekor (ADR 0010-adjacent transparency work), the provenance record
is itself tamper-evident. If an agent later performs a harmful action, an
investigator can identify the exact frame that induced it and verify, by hash,
that a suspected screenshot is or is not the one the agent actually saw.

## Why this is the right layer

The gate, not the model, is Miru's trust boundary (consistent with ADR 0011's
context-aware enforcement). Provenance is enforcement-layer evidence: it does
not depend on the model behaving correctly, cannot be talked out of recording by
an injected instruction, and composes with the existing redaction matrix (the
provenance fields are not redacted; the image bytes themselves are never written
to the log, only their digest).

## Consequences

+ VPI attacks become post-hoc traceable and hash-verifiable without any VLM or
  detection heuristic that an adaptive attacker could evade.
+ Negligible cost: one SHA-256 over an already-allocated PNG buffer, one
  append to an existing log.
+ The digest is returned to the agent, enabling future client-side display of
  "Claude is viewing frame X" for in-context user consent (arXiv:2504.17934).
− Provenance is evidence, not prevention; it does not stop a VPI-induced action
  in the moment. Prevention for *Dangerous* actions still relies on the
  confirmation gate (ADR 0011). The two compose: Conditional/Normal actions are
  traceable; Dangerous ones are both traceable and gated.

## References

- arXiv:2506.02456 (VPI-Bench: Visual Prompt Injection for Computer-Use Agents)
- arXiv:2510 "The Attacker Moves Second" (12 defenses bypassed >90%)
- arXiv:2504.17934 (human-centered evaluation / in-context consent)
- ADR 0011 (context-aware security levels)
- `crates/miru-mcp/src/server.rs` (`tool_screen_capture`)
- `crates/miru-agent/src/session.rs` (`audit_screen_capture`)
