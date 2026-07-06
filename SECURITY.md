# Security Policy

## Reporting a vulnerability

**Preferred channel** — GitHub Private Vulnerability Reporting:
[https://github.com/shizukutanaka/miru/security/advisories/new](https://github.com/shizukutanaka/miru/security/advisories/new)

This gives us a private workspace to triage, draft a fix, and assign a
CVE through GitHub's Numbering Authority — all coordinated with you.

**Email backup** — not yet available. A PGP-encrypted email channel
(`security@miru.app`) is planned but not yet set up: there is no
monitored inbox and no PGP key published. Until this is stood up,
**use the GitHub Private Vulnerability Reporting channel above as the
only channel** — do not send reports to `security@miru.app` expecting
a timely (or any) response yet. This section will be updated with a
real fingerprint once the key exists and the inbox is monitored.

### What we commit to

- **Acknowledge** within 72 hours (in practice usually within 24).
- **Initial severity assessment** within 5 business days (CVSS v3.1).
- **Fix or mitigation plan** within 14 days for critical, 30 for high.
- **Coordinated public disclosure** within 90 days of the report, with
  a possible 14-day extension on request. We will pre-notify major
  distributors (Homebrew, AUR, Flatpak) before the public release.
- **Credit** in the GHSA, the CHANGELOG, and `docs/postmortems/`,
  unless you prefer to remain anonymous.

### What helps us

- A clear reproduction (PoC code, screenshots, video). Even a flaky
  reproduction is more useful than none.
- The Miru version (output of `miru-host --version`) and OS.
- An assessment of impact in your view: who can exploit it, with what
  prerequisites, what is the worst-case outcome.

## Scope

The Miru source tree is in scope:

- `miru-host`, `miru-client`, `miru-mcp`, `miru-signal` binaries
- All crates under `crates/` published as part of a release
- Build infrastructure: GitHub Actions workflows, release pipeline
- The protocol specification in `docs/CRYPTO.md`

Out of scope:

- Third-party MCP clients (Claude Desktop, Cursor, etc.) — report to them.
- Third-party Rust crate vulnerabilities — report to RustSec; we will
  bump our `Cargo.lock` once a patched release exists.
- Issues in pre-1.0 features explicitly documented as "experimental".
- Denial-of-service via resource exhaustion at scales below 10 reqs/sec
  (we accept rate-limit guard rails as the mitigation).

## Supported versions

| Version | Supported          |
|---------|--------------------|
| `main`  | :white_check_mark: |
| 0.1.x   | :white_check_mark: |
| < 0.1   | :x:                |

We support exactly two minor branches at any time: the current and the
previous. Critical fixes are backported to both.

## Threat model and crypto specification

Read these alongside this document:

- [`docs/THREAT_MODEL.md`](./docs/THREAT_MODEL.md) — STRIDE per component,
  with explicit "out of scope" sections.
- [`docs/CRYPTO.md`](./docs/CRYPTO.md) — every algorithm, every parameter,
  every invariant the protocol depends on.

## Hall of fame

We thank the following researchers for responsible disclosure:

*(none yet; you could be first.)*

## Honesty about residual risk

The hardest open problem in this product class is **indirect prompt
injection via screen content** delivered to an AI agent. Anthropic's
Sonnet 4.5 / Opus 4.5 system cards report meaningful improvements (88%
blocked baseline, 1.4% adaptive-attack success post-mitigation) but not
solved. Our deterministic mitigations:

- Capability-scoped tokens with TTL ≤ 15 minutes.
- Per-tool rate limits + 24-hour daily caps.
- Human-confirmation gate for destructive capabilities (`ShellExec`,
  `FileWrite`, `OpenUrl`, long `KeyType`).
- Hash-chained audit log of every tool call with redacted arguments.
- Parent-process allowlist on the MCP server.

These do not depend on the LLM correctly refusing a malicious instruction.
If a reviewer finds a path to bypass them, that is a security issue we
want to hear about.
