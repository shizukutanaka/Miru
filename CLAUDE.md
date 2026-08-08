# Miru — リモートデスクトップ CLAUDE.md

## WHY
TeamViewer/AnyDesk代替。完全セルフホスト可能、E2E暗号化、P2P優先。
Rust + Tauri v2 でメモリ安全 + 軽量バイナリ。

## WHAT
- **Host**: 画面キャプチャ + AV1/VP9エンコード + 入力受信
- **Viewer**: Tauri v2 + React UI、デコード・WebGL2表示・入力送信
- **Signal**: ランデブー(NAT越え) + リレー(P2P失敗時フォールバック)

## MAP
```
miru/
├── crates/
│   ├── miru-common/    プロトコル定義・X25519/ChaCha20暗号
│   ├── miru-capture/   OS別キャプチャ (DXGI/CGDisplay/X11/PipeWire)
│   ├── miru-codec/     VP9 (libvpx) + JPEG + HW probe + ffmpeg雛形
│   ├── miru-audio/     Opus エンコード/デコード/再生
│   ├── miru-auth/      Ed25519 identity + PIN + TOFU + ACL
│   ├── miru-transport/ QUIC + WebSocket relay + STUN/hole-punch + handshake
│   ├── miru-input/     SendInput/CGEvent/uinput
│   ├── miru-host/      ホストバイナリ (capture, QoS, session, banner)
│   ├── miru-client/    Tauri viewer (Rust + React/TS + WebGL2)
│   └── miru-signal/    ランデブー + リレーサーバー (Docker)
├── docs/
│   ├── architecture.md
│   ├── protocol.md     プロトコル仕様 v1
│   ├── roadmap.md      30%向上 + 10年運用ロードマップ
│   └── adr/            アーキテクチャ決定記録
├── fuzz/               cargo-fuzz ターゲット
├── .github/workflows/  CI/CD
├── scripts/            install.sh + install.ps1
├── SECURITY.md
└── CONTRIBUTING.md
```

## RULES
- `unsafe` は `platform/` 配下のみ
- 各 crate は単一責務 (SRP)
- `unwrap()` 禁止 — `?` または `anyhow::bail!`
- テスト: 機能追加と同 PR で必須
- 破壊的変更は major version のみ、6か月の `#[deprecated]` 期間
- コミット: Conventional Commits (feat/fix/perf/refactor/test/docs/chore)

## WORKFLOWS

### 開発
```bash
cargo test --workspace
cargo run -p miru-signal           # シグナル (ports 21115, 21117)
MIRU_SIGNAL=ws://localhost:21115/ws cargo run -p miru-host
cd crates/miru-client/ui && npm install && npm run dev
cd crates/miru-client && cargo tauri dev
```

### テスト全種
```bash
cargo test --workspace                                  # 全テスト
cargo test -p miru-host --test loopback -- --ignored    # E2E
cargo bench -p miru-common                              # 性能ベンチ
cargo +nightly fuzz run fuzz_message_parse              # ファズ
cargo audit                                              # 脆弱性スキャン
cargo clippy --all-targets -- -D warnings               # lint
```

### Docker セルフホスト
```bash
MIRU_PUBLIC_HOST=your.server.ip docker-compose up -d
```

### リリース
```bash
git tag v0.1.0 && git push origin v0.1.0
# → GitHub Actions が全OSビルド + リリース作成
```

## 設計思想
- **Carmack**: ゼロコピー (DXGI GPU texture → HW encoder直接)、SIMD色変換
- **Martin**: SRP, 各 crate 単一責務、依存方向は外向き
- **Pike**: feature flag より型分岐、明示的エラー、シンプルさ優先

## 暗号
- 鍵交換: X25519 ECDH (ephemeral, PFS)
- 対称: ChaCha20-Poly1305 (96-bit nonce, monotonic counter, replay保護)
- identity: Ed25519 (TOFU + 16桁hex指紋表示)
- PIN: PBKDF2-SHA256 (100k iter, salt=device_id)
- 将来: ML-KEM-768 ハイブリッド (X25519MLKEM768、旧称 Kyber768。docs/RESEARCH_NOTES.md §4)

## ポート
| Port  | Proto | 用途                  |
|-------|-------|-----------------------|
| 21115 | TCP   | ランデブー WebSocket  |
| 21116 | UDP   | QUIC ランデブー (将来) |
| 21117 | TCP   | リレー WebSocket      |

## 状態 v0.1.0
**10 crates, ~96ファイル, ~9000行**

- ✓ プロトコル v1 完全定義
- ✓ X25519 ハンドシェイク実動作 (テスト済み)
- ✓ libvpx VP9 enc/dec 動作
- ✓ JPEG 経由のビューア表示パス
- ✓ WebGL2 YUV シェーダー (高速パス準備済み)
- ✓ Tauri v2 ビューア UI (接続・セッション画面・ペアリング)
- ✓ Ed25519 identity + TOFU ACL + PIN
- ✓ NAT 越え (STUN + UDP hole-punch)
- ✓ シグナル + リレーサーバー (Docker)
- ✓ 全OS抽象 (Win DXGI / macOS CGDisplay / Linux X11)
- ✓ 13個の統合テスト + ベンチ + ファズ
- ✓ ADR 3件、SECURITY.md、CONTRIBUTING.md

### 残作業 (Sprint 4以降)
- [ ] ffmpeg-next 実コード化 → AV1 HW
- [ ] WebGL2 YUV パスを実通信フローで動作 (現在 JPEG プレビュー)
- [ ] PipeWire DMA-BUF 実装 (Wayland zero-copy)
- [ ] PIN ペアリング UI と Rust 側の統合
- [ ] 自動更新 (tauri-plugin-updater)
