# Miru Incident Response Runbook

This runbook is the operational playbook for security incidents. It is
maintained alongside [`SECURITY.md`](../SECURITY.md) (the user-facing
policy) and [`THREAT_MODEL.md`](./THREAT_MODEL.md) (the design-level
threat enumeration).

The intended audience is whoever is on-call when something goes wrong:
the maintainer, a co-maintainer, or — in the future — a coordinated
response team.

## Severity ladder

| Severity   | Definition                                                                | Response time |
|------------|---------------------------------------------------------------------------|---------------|
| **P0**     | Active exploitation in the wild OR cryptographic break OR signing-key compromise | < 4 hours     |
| **P1**     | Pre-exploit RCE, privilege escalation, or unauth disclosure of secrets    | < 24 hours    |
| **P2**     | Authenticated data exposure, MITM possible under non-default config       | < 7 days      |
| **P3**     | DoS, info-leak that doesn't reveal user data, hardening regressions       | < 30 days     |

## Response phases

### Phase 1 — Triage (first hour)

1. **Acknowledge** the report on the channel it arrived on (GHSA draft,
   email reply). One sentence is enough; full triage comes later.
2. **Assign** a severity (above ladder). When uncertain, choose the
   higher severity; downgrade is cheaper than upgrade.
3. **Open a private GHSA draft** if the report came in by email. Pull
   the reporter in as a collaborator on the draft.
4. **Decide on the timeline**: 90-day default, shorter for active
   exploitation. Communicate to reporter.

### Phase 2 — Reproduce (first day)

1. Reproduce locally with the reporter's PoC. If reproduction fails
   on your environment, confirm OS/version with the reporter; do not
   close as "cannot reproduce" without exhausting environment differences.
2. Write a **regression test** that captures the bug. Commit to the
   private security branch (not main yet) — this is the durable
   artefact that prevents recurrence.
3. Identify the **blast radius**: which versions are affected, which
   defaults trigger it, what prerequisites the attacker needs.

### Phase 3 — Fix (severity-dependent timeline)

1. Branch off the private security workspace. Commit the fix.
2. Verify the regression test now passes.
3. **Code review**: at least one second pair of eyes for P0/P1.
   For a single-maintainer project, this means asking a trusted
   peer; document who reviewed and when.
4. **Backport** to all supported branches (current + previous minor).

### Phase 4 — Pre-release coordination (P0/P1 only)

For high-severity issues, pre-notify distributors so patched binaries
land simultaneously with the public disclosure. Notify by email with
a draft GHSA link and an embargoed release date:

- Homebrew tap maintainer
- AUR package maintainer
- Flatpak / Snap maintainer (when applicable)
- Any major downstream user who has asked for security pre-notification

Embargo length: typically 7 days for critical, 0–3 for important.

### Phase 5 — Public release

1. Push patched versions to all supported branches simultaneously.
2. Publish the GHSA, request a CVE through GitHub's CNA workflow.
3. Update CHANGELOG with the GHSA ID and a one-line summary.
4. Tweet / post / email the security mailing list.

### Phase 6 — Post-mortem (within 14 days)

Write `docs/postmortems/YYYY-MM-DD-<slug>.md` covering:

- What happened (chronology in UTC)
- Why our existing defences didn't catch it
- Root cause
- Customer impact (your honest estimate)
- Remediation (the patch)
- **Process changes** so the same class cannot recur:
  - new tests
  - new CI checks
  - new threat-model sections
  - new monitoring

The post-mortem is public unless the report specifically requests
otherwise. Public post-mortems serve the rest of the ecosystem.

## Specific scenarios

### Scenario A: Cryptographic break in our code

Examples: nonce-reuse discovered in shipping versions, Ed25519
verification bug, Argon2 parameter regression.

1. Treat as **P0** unconditionally.
2. Disable affected functionality at runtime if possible (via remote
   kill switch — we don't have one as of v0.1; this is a v1.0 item).
3. Cut a patched release within 4 hours; backport.
4. Force re-pairing of all peer pinning if the break affects identity.
5. Public post-mortem includes the cryptographer review process that
   missed it, and what is changing.

### Scenario B: Signing-key compromise

Examples: GitHub release-signing key leaked, crates.io trusted-publisher
identity stolen.

1. **Immediately** revoke the affected signing identity.
2. Re-issue keys; tag and sign a new release that supersedes all
   compromised versions.
3. Publish a security advisory that lists every release signature that
   should NOT be trusted. Update the README install instructions.
4. Post on social channels and the GitHub Discussions security board.
5. Coordinate with crates.io / GitHub if their infrastructure was
   involved.

### Scenario C: Active exploitation in the wild

A user reports they were attacked using a specific Miru version.

1. Severity = P0.
2. **Privately** request the user's logs (audit log, OS event log,
   network captures) — these may contain forensic detail invaluable
   to other potential victims.
3. Dispatch the patch within 4 hours per the P0 SLA.
4. Coordinate with the user on disclosure timing. They may want
   anonymity; respect that.
5. Note in the post-mortem that real-world exploitation occurred —
   this is important for the broader community to size their response.

### Scenario D: Dependency vulnerability disclosure

RustSec advisory affects a transitive dep we use.

1. Severity follows the upstream advisory's CVSS, capped by whether
   our usage actually exercises the vulnerable code path.
2. If patched upstream version exists: bump `Cargo.lock`, run tests,
   ship a patch release. No GHSA of our own needed (the upstream
   advisory is the canonical record).
3. If no patched version: evaluate whether to drop the dep, vendor a
   patched fork, or accept the risk with documentation. Update
   `THREAT_MODEL.md` Section 6 ("Out of scope") if accepting.

### Scenario E: Build-pipeline compromise (xz-utils style)

Suspect that a build script in a transitive dep contains malicious code.

1. Severity = P0 if shipped, P1 if caught pre-release.
2. Halt all releases until the dep tree is audited.
3. Run `cargo vet diff <crate>` against a known-good version.
4. If confirmed: yank affected releases, force re-key of any signing
   keys that touched the compromised build.
5. Post-mortem must address how the dep got past `cargo deny` /
   `cargo vet` / the 7-day cooldown policy.

## Communication templates

### Initial acknowledgement (within 72h)

> Thanks for the report. Acknowledged at <timestamp UTC>. Tracking as
> draft GHSA-xxxx-xxxx-xxxx; you've been added as a collaborator. We
> will provide a severity assessment and timeline within 5 business
> days. Coordinated disclosure target: 90 days from today
> (<calculated date>).

### Severity assessment

> Severity assessment for GHSA-xxxx-xxxx-xxxx:
>
> CVSS 3.1: <score> (<vector>)
> Severity: <P0/P1/P2/P3>
> Affected versions: <range>
> Fixed in: TBD (target date <X>)
> Prerequisites for exploitation: <list>
> Worst-case impact: <one paragraph>

### Public release announcement

> Miru <version> released addressing GHSA-xxxx-xxxx-xxxx
> (<one-sentence summary>).
>
> CVE: <id>
> Severity: <CVSS>
> Reporter: <handle, with thanks>
>
> All users on <affected range> should upgrade. Detail and
> post-mortem: <link>.

## Post-mortem template

`docs/postmortems/YYYY-MM-DD-<slug>.md`:

```markdown
# Incident YYYY-MM-DD <slug>

- Severity: <CVSS>
- Discovered by: <reporter>
- Disclosed: <date>
- Affected versions: <range>
- Fixed in: <ver>
- CVE: <id>
- GHSA: <id>

## What happened
<Chronology in UTC, plain language.>

## Why our existing defences didn't catch it
<Honest. The point is to learn.>

## Timeline (UTC)
- <T+0>:  reporter opened private advisory
- <T+1h>: maintainer acknowledged
- ...

## Customer impact
<Your honest estimate. "Unknown but bounded by …" is acceptable.>

## Root cause
<Single sentence followed by detail.>

## Remediation (shipped)
<What landed, with commit links.>

## Process changes
- Test added: <link>
- CI check added: <link>
- Threat model section updated: <link>

## Acknowledgments
<Reporter, reviewers, distributors, …>
```

## On-call cheat sheet

When something is on fire, in this order:

1. **Acknowledge.** One sentence. Don't fix yet.
2. **Triage severity.** Use the ladder above.
3. **Reproduce.** Don't proceed to a fix without a reproduction.
4. **Fix on a private branch.** Tests first.
5. **Backport.** All supported minor branches.
6. **Pre-notify distributors** (P0/P1 only).
7. **Release publicly** with the GHSA at the same moment.
8. **Post-mortem within 14 days.** Public.

If you forget every other step, do at least #1 and #3. They prevent
the worst failure modes (silence and misdiagnosis).
