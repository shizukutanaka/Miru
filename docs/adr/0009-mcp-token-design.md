# ADR-0009: MCP capability トークン設計 (TTL ≤15分, per-capability scoping)

- Status: Accepted
- Date: 2026-05
- Deciders: shizukutanaka

## Context

AI agent (Claude / Cursor / Codex) に PC 操作を許可するときのトークン設計。
以下の制約がある:

1. **LLM は信頼できない** — prompt injection で挙動が変わり得る
2. **MCP token は流出しやすい** — Claude Desktop の設定ファイルは平文
3. **操作の影響範囲が大きい** — ShellExec は任意コード実行に相当
4. **AnyDesk 2024 / RustDesk CVE-2026 教訓** — credential 流出 → 全接続が危険

## Decision

**Ed25519 署名付き ability-scoped トークン、TTL hard cap 15 分**

```
miru-agent.<base64url(payload)>.<base64url(sig)>
```

payload:
- `jti` (UUID) — revocation lookup
- `iss` (Ed25519 pubkey, base64) — 発行者
- `sub` (label) — 「claude-code-session」等
- `iat` / `exp` (unix sec) — 発行/有効期限
- `caps` (capability set) — 許可される操作のみ
- `scope` (optional) — プロセス/ウィンドウ制限

**TTL hard cap = 900秒 (15分)**。発行側が 30日 TTL を要求しても、
`AgentToken::issue` が 900 秒に clamp する。コンパイル時定数 `MAX_TTL_SECS`。

**Destructive cap (ShellExec / FileWrite / OpenUrl / 長いKeyType) は人間確認必須**。
確認なしの authorize は deny。

**Rate limit** — per-capability token bucket + 24h daily cap。
ShellExec 1/分 30/日、ScreenRead 10/秒 1000/日。

**Revocation list** — `RevocationList::revoke(jti, reason)` → append-only log。
panic ボタンで全 JTI を一括 revoke。

**Parent process allowlist** — MCP server 起動時に親プロセスが
Claude Desktop / Cursor / Codex / Continue であることを確認。それ以外は拒否。

## Consequences

**正**:
- 15分 TTL により流出トークンの有効期間を最大 15分に制限
- capability scoping により「ScreenRead トークン」で ShellExec 不可
- rate limit により runaway agent ループを防止
- parent allowlist により malware が直接 miru-mcp を起動できない

**負**:
- TTL が短いため、長時間の AI agent セッション (30分超) ではユーザーが再発行必要
- CLI からの直接テストには `--insecure-no-parent-check` が必要

**将来**:
- `scope` フィールドに user-task ID を埋め込み → confused deputy 防止 (v1.0)
- Periodic Rekor anchor で audit log tip を外部に記録 (v1.0)
- Hybrid PQC (X25519 + ML-KEM-768) で harvest-now-decrypt-later 耐性 (v2.0)
