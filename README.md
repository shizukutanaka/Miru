# Miru (見る)

> AI 時代の安全な PC 制御 fabric — P2P リモートデスクトップ + AI エージェント基盤

[![CI](https://github.com/shizukutanaka/miru/actions/workflows/ci.yml/badge.svg)](https://github.com/shizukutanaka/miru/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-173%20passing-brightgreen.svg)](#状態)
[![Build](https://img.shields.io/badge/workspace-17%2F17%20compile-brightgreen.svg)](#状態)
[![Clippy](https://img.shields.io/badge/clippy-0%20warnings-brightgreen.svg)](#状態)

## なぜ Miru か

リモートデスクトップは 20 年間ほぼ変わっていない。Miru は 4 軸で刷新する:

### 1. AI エージェント First Class
- Capability-scoped Ed25519 トークン (12 capability / TTL ≤15 分 / hard cap)
- 破壊的操作 (ShellExec / FileWrite / OpenUrl) は TOFU 確認ゲート
- Per-capability rate limit + 24h daily cap (ShellExec: 1/分 30/日)
- 改ざん検知 audit log + redaction matrix (KeyType → SHA-256 + len)
- Parent process allowlist (Claude Desktop / Cursor / Codex のみ起動可)
- Panic rotation: revocation list で全トークンを即時無効化

### 2. Verifiable End-to-End
- CoSignedCommitment (Ed25519 双方署名) + ローカル Merkle log
- Sigstore Rekor 互換の透明性ログ
- 後から「このセッションは改ざんされていない」を第三者証明可能

### 3. Device Constellation
- 全デバイスを Ed25519 署名グラフで束ねる
- Capability 広告 + mDNS discovery (中央サーバー不要)

### 4. Temporal (Time Travel)
- セッションを deterministic 記録 → スクラブ / 倍速 / ジャンプ

## 状態

```
Workspace:        17/17 crates compile (rustc 1.88, --all-targets)  ✅
Warnings:         0 (cargo + clippy + rustfmt)                      ✅
Tests:            173 passing / 0 failing (unit + integration + doc) ✅
Sandbox:          seccomp syscall=true  ✅  (実機確認)
Signal register:  Register: XXXX-XXXX  ✅  (実機確認)
```

### クレート一覧

| Crate | 役割 | Tests |
|-------|------|-------|
| `miru-agent` | Capability token / audit / redact / revocation | 44 |
| `miru-common` | Protocol / Crypto (AEAD + replay) | 36 |
| `miru-auth` | Ed25519 / Argon2id / TOFU log | 26 |
| `miru-mcp` | MCP server (stdio) | 21 |
| `miru-host` | Host daemon + token CLI | 18 |
| `miru-transparency` | Merkle + Rekor | 10 |
| `miru-constellation` | Device graph | 5 |
| `miru-transport` | QUIC + WS relay + handshake | 5 |
| `miru-sandbox` | landlock + seccomp + macOS/Win scaffolding | 4 |
| `miru-audio` | Opus 1/2/5.1/7.1 | 3 |
| `miru-signal` | Rendezvous + relay server | 1 |
| `miru-codec` | VP9 (feature `vpx`) + JPEG fallback + HW stubs | — |
| `miru-capture` | DXGI / SCK / X11 SHM | — |
| `miru-input` | SendInput / CGEvent / uinput | — |
| `miru-discovery` | mDNS | — |
| `miru-client` | Tauri viewer | — |
| `miru-bench` | Criterion benchmarks | — |

## セキュリティ

すべて [`docs/CRYPTO.md`](docs/CRYPTO.md) + [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) で仕様化。

| 項目 | 内容 |
|------|------|
| Key exchange | X25519 ephemeral (PFS) |
| AEAD | ChaCha20-Poly1305 / per-direction subkey / 128-bit replay window |
| Identity | Ed25519 + `verify_strict` (ZIP-215) — CI gate で非 strict 禁止 |
| PIN | Argon2id m=64MiB t=3 p=1 — RustDesk CVE-2026-30785 対策 |
| TLS | Full validation 強制 — CVE-2026-30794 対策 |
| TOFU | Hash-chained pin log — `miru-auth::fingerprint_log` |
| Token | Ed25519 signed / TTL ≤15min / revocation / rate limit |
| Sandbox | Linux: landlock + seccomp / macOS: hardened runtime / Windows: AppContainer |

## クイックスタート

```bash
# Linux 依存
sudo apt install libx11-dev libxcb-shm0-dev libopus-dev libvpx-dev \
                 libasound2-dev libgtk-3-dev pkg-config

# ビルド (rustc ≥ 1.88 — rust-toolchain.toml が自動選択)
cargo build --workspace --release

# シグナルサーバー
cargo run -p miru-signal

# ホスト (別ターミナル)
MIRU_SIGNAL=ws://localhost:21115/ws cargo run -p miru-host

# ヘッドレス / CI: Xvfb 使用
Xvfb :99 -screen 0 1920x1080x24 &
DISPLAY=:99 MIRU_SIGNAL=ws://localhost:21115/ws cargo run -p miru-host
```

### Claude Desktop と統合

まず capability トークンを発行する (host の identity で署名される):

```bash
# 安全なデフォルト (画面読取 + ポインタ + タイプ、shell/file-write なし)
miru-host token issue

# capability と TTL を指定
miru-host token issue --cap screen_read,pointer_click,key_type --ttl-hours 4
```

出力されたトークンを `~/Library/Application Support/Claude/claude_desktop_config.json` に設定:

```json
{
  "mcpServers": {
    "miru": {
      "command": "/usr/local/bin/miru-mcp",
      "env": {
        "MIRU_AGENT_TOKEN": "miru-agent.<発行されたトークン>",
        "DISPLAY": ":0"
      }
    }
  }
}
```

`$DISPLAY` を設定すると **実際のスクリーンをキャプチャして Claude に送れる**。
破壊的 capability (shell_exec/file_write) は明示指定が必要で、実行時にも確認ゲートを通る。

## 監査ログの検証

AI エージェントの全操作は改ざん検知可能な hash-chain で記録される。
オフラインで整合性を検証できる:

```bash
# hash-chain 整合性を検証 (改ざん/切り詰めを検出)
miru-host audit verify

# 全エントリを表示 (検証してから出力)
miru-host audit show
```

改ざんされたログは `✗ Audit chain BROKEN` で exit 1。tip (最新エントリ) は
Rekor anchoring で外部固定される (ADR 0010 周辺)。

## ドキュメント

| ファイル | 内容 |
|----------|------|
| [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) | STRIDE per component |
| [`docs/CRYPTO.md`](docs/CRYPTO.md) | 全暗号 invariant + handshake transcript |
| [`docs/INCIDENT_RESPONSE.md`](docs/INCIDENT_RESPONSE.md) | P0-P3 + 5 scenarios |
| [`SECURITY.md`](SECURITY.md) | 報告手順 + PGP + 90日 SLA |
| [`docs/UNWRAP_TRIAGE.md`](docs/UNWRAP_TRIAGE.md) | 24 unwrap 全件分類 |
| [`docs/adr/`](docs/adr/) | ADR 0001-0012 (設計判断記録) |
| [`docs/PRODUCT_REVIEW.md`](docs/PRODUCT_REVIEW.md) | v0.1.0 長所短所レビュー + 改善ロードマップ |

## ライセンス

MIT — 商用利用・統合 OK。
