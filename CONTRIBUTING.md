# Contributing to Miru

## 開発開始

```bash
git clone https://github.com/shizukutanaka/miru
cd miru

# Linux 依存
sudo apt install libx11-dev libxcb-shm0-dev libxcb-randr0-dev \
    libasound2-dev libpulse-dev pkg-config

cargo build --workspace
cargo test --workspace

# Run signal server (terminal 1)
cargo run -p miru-signal
# Run host (terminal 2)
MIRU_SIGNAL=ws://localhost:21115/ws cargo run -p miru-host
# Run viewer UI (terminal 3)
cd crates/miru-client/ui && npm install
cd .. && cargo tauri dev
```

## コーディング規約

- `cargo fmt` 必須
- `cargo clippy --all-targets -- -D warnings` パス必須
- `unwrap()` 禁止 — `?` または `anyhow::bail!`
- `unsafe` は `platform/` 内のみ
- 全 public 関数に doc comment

## コミット

プレフィックス: `feat:`, `fix:`, `perf:`, `refactor:`, `docs:`, `test:`, `chore:`

例: `feat(transport): add UDP hole punching for symmetric NAT`

## PR チェックリスト

- [ ] `cargo fmt && cargo clippy` クリーン
- [ ] `cargo test --workspace` パス
- [ ] 新機能には test 追加
- [ ] 破壊的変更時は CHANGELOG に記載
- [ ] アーキテクチャに影響する変更は `docs/adr/` に ADR 追加

## ADR (Architecture Decision Record)

`docs/adr/NNNN-title.md` フォーマット:

```markdown
# ADR-NNNN: タイトル
Date: YYYY-MM-DD
Status: Accepted

## Context
## Decision
## Consequences
```

## クレート責務

| Crate | 責務 |
|-------|-----|
| miru-common | プロトコル型、暗号、共有定数 |
| miru-capture | キャプチャ抽象 |
| miru-codec | 動画コーデック |
| miru-audio | Opus + 再生 |
| miru-transport | QUIC/Relay/シグナル/NAT |
| miru-input | OS 入力 |
| miru-auth | identity, PIN, ACL |
| miru-host | ホスト実行ファイル |
| miru-client | ビューア (Tauri + React) |
| miru-signal | ランデブー + リレー |
| miru-bench | ベンチマーク |

## セキュリティ報告

GitHub Issue ではなく `security@miru.app` (将来) に報告してください。
90日の responsible disclosure ポリシー。
