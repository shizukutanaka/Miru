# ADR 0011 — Context-Aware Capability Security Levels

Status: Accepted
Date: 2026-05-30

## Context

Miru's MCP confirmation policy (ADR 0009, Sprint 18) was binary: a capability
either auto-approves or auto-denies. CSAgent (Gong et al., "Secure and Efficient
Access Control Framework for Computer-Use Agents via Context Space",
arXiv:2509.22256) makes a sharper point backed by measurement:

- Per-action user confirmation is the industry default (OpenAI Operator, Claude
  Computer Use), but it **causes alert fatigue** — users approve habitually
  without reading (CSAgent §3.1; corroborated by arXiv:2509.10723 "Dark Patterns
  Meet GUI Agents", which finds human oversight has "limited efficacy").
- The same action is safe in one context and dangerous in another. File
  deletion with an explicit target path is fine; the same call with no target
  is destructive. A binary classifier cannot express this.
- CSAgent's taxonomy: every function is `normal` / `conditional` / `dangerous`.
  Their context-aware enforcement blocks 99.36% of attacks with only 6.83%
  latency overhead and 9.33% utility loss — far better than confirm-everything.

The related GUI-agent threat literature (arXiv:2504.11281 fine-print injection,
arXiv:2504.17934 human-centered evaluation) reinforces that agents do not
reliably self-protect, so the *gate*, not the model, must enforce safety.

## Decision

Introduce `SecurityLevel { Normal, Conditional, Dangerous }` and classify every
`Capability` via `Capability::security_level()`:

- **Normal** — `ScreenRead`, `PointerMove`. Read-only observation; never prompts.
- **Conditional** — `PointerClick`, `KeyType`, `KeyCombo`, `ClipboardWrite`,
  `OpenUrl`. Safe during active interactive use; a context-aware gate MAY check
  context (e.g. is the user present?) before allowing, but does not prompt on
  every action.
- **Dangerous** — `ClipboardRead`, `FileRead`, `FileWrite`, `ShellExec`.
  Irreversible or sensitive; always requires explicit confirmation.

`requires_confirmation()` is now defined as `security_level() == Dangerous`, and
an invariant test (`dangerous_level_implies_confirmation`) keeps the two
definitions in sync across the whole `Capability::all()` set.

## Why not full CSAgent context spaces now

CSAgent's full design generates per-application JSON "context spaces" with an
LLM analyzer and a runtime RPC service extracting intents. That is a large
subsystem appropriate for an OS-level product. For Miru v0.1 the three-level
classification captures the most valuable 80%: it lets the MCP gate stop
prompting for `Conditional` actions during normal use (killing alert fatigue)
while still hard-gating `Dangerous` ones. Full intent-aware context spaces are a
candidate for v1.0 when the MCP↔host channel carries richer session context.

## Consequences

+ The gate can be tuned per security level rather than per individual
  capability, and the classification is centralized and tested.
+ Directly addresses the alert-fatigue failure mode that the literature shows
  defeats confirm-everything designs.
+ `SecurityLevel` is serializable, so it can travel in audit-log entries and
  future capability certificates.
− Still coarser than CSAgent's per-intent context rules; `Conditional` actions
  are currently allowed without an actual context check (the hook exists, the
  context signal — user-presence, foreground app — is wired in v1.0).

## References

- arXiv:2509.22256 §3.1 (security-usability trade-off), §4.2 (Contextual
  Integrity taxonomy: normal/conditional/dangerous)
- arXiv:2509.10723 (human oversight limited efficacy against dark patterns)
- arXiv:2504.17934 (human-centered evaluation, in-context consent)
- ADR 0009 (MCP token design), ADR 0010 (replay protection)
- `crates/miru-agent/src/token.rs` (`SecurityLevel`, `security_level()`)
