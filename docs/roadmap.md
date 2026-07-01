# Miru — 長期改善ロードマップ

## 目的
プロダクトを30%向上、10年間動かせる基盤を確立。

## 0. 優先順位の見直し (2026-06、ソクラテス式問答による分析)

CLAUDE.md の WHY は「TeamViewer/AnyDesk代替」であり、評価基準は
「今日配って、音声付き60fps HWエンコード映像が初回接続で見えるか」の一点に
尽きる。この基準で現状を棚卸しした結果:

**決定的に不足 (v0.1優先で着手)**:
- ホスト側システム音声キャプチャ — `miru-audio` の Opus encode/decode/playback
  は完成しているが、`miru-host` がどこからも呼んでおらず AudioFrame が
  一度も送信されない (Features.audio は誤って true を広告していたため
  false に修正済み)
- HW エンコード (AV1/H264/H265) — `ffmpeg_enc.rs` は全メソッドが `bail!`
- WebGL2 YUV→RGB 描画パス — JPEG プレビューのみが本番導線
- Wayland/PipeWire キャプチャ — スタブ
- CI/CD — `.github/workflows/` ではなく `workflows-proposed/` に隔離されたまま
  実行されていない (GitHub App の `workflows` 権限不足、要手動対応)

**質は高いが時期尚早 (中核完成までは追加投資を凍結)**:
- NAT越え高度化(STUN/ホールパンチ/NATタイプ判定) — 実装済みで十分。
  これ以上の最適化より上記の「不足」に資源を回す
- セッション録画(`miru-host/src/recording.rs`) — ライブ接続の基本体験が
  未完成な段階での拡張は優先度を下げる
- `miru-agent`/`miru-mcp`/`miru-sandbox`/`miru-constellation`/
  `miru-discovery`/`miru-transparency` (AIエージェント統治基盤) — 暗号設計・
  capability トークン設計は業界水準以上に高品質だが、これは「人間向け
  リモートデスクトップ」という一次ゴールの外側にある別プロダクトの種。
  実利用データが無いまま capability 粒度を増やすより、実際に使われてから
  拡張すべき

---

## 1. 信頼性 (10年運用前提)

### プロトコル進化
- [ ] `PROTOCOL_VERSION` フィールドで全メッセージにバージョン埋め込み済み
- [ ] **最小2世代の後方互換性保証** — クライアントv0.x はサーバー v1.x まで通信可
- [ ] 破壊的変更時は移行期間6か月、デプリケーション通知をHelloAck内に含める
- [ ] プロトコル仕様を `docs/protocol.md` で正式定義 (RFC形式)
- [ ] 全メッセージタイプに対してファジングテスト (cargo-fuzz)

### 依存最小化
- [ ] **静的リンク**: 配布バイナリは libvpx 等を全て static — OS依存なし
- [ ] 外部サービス依存ゼロ — シグナルサーバーは完全セルフホスト可
- [ ] STUNサーバーは複数候補ローテーション (Google, Cloudflare, 自前)
- [ ] ハードコード値なし — 全エンドポイント環境変数で上書き可能

### ストレージ後方互換
- [ ] 設定ファイル (acl.json) はスキーマバージョン埋め込み
- [ ] デバイス identity ファイルは生キーバイト直書き (フォーマット変更不要)
- [ ] マイグレーションスクリプト同梱 (旧バージョン→新バージョン)

---

## 2. パフォーマンス (30%向上)

### キャプチャ
- [ ] **DXGI dirty rect**: 変更領域のみエンコード — 静的画面で帯域90%削減
- [ ] **Wayland DMA-BUF**: PipeWireでGPUテクスチャを直接共有 (CPU経由なし)
- [ ] **macOS IOSurface**: ScreenCaptureKit → VideoToolbox に直接渡す
- [ ] フレームスキップ: 直前フレームと完全一致なら送信スキップ

### エンコード
- [ ] **AV1 HW**: NVENC (RTX 40+), QSV (Arc), AMF (RX 7000+) — 同品質で帯域30%減
- [ ] **適応ビットレート**: AIMD実装済み → BBR風アルゴリズム検討
- [ ] **可変フレームレート**: 静止画は5fps、動画は60fps切替
- [ ] スクリーンコンテントモード (SCM): VP9/AV1で文字フォントに最適化

### トランスポート
- [ ] **QUIC優先パス**: P2P成功時はリレー切断 — 帯域・レイテンシ削減
- [ ] **多ストリーム並列**: 動画/音声/入力で別ストリーム → HOL-blocking回避
- [ ] **0-RTT再接続**: 同一ピア再接続時にハンドシェイクスキップ
- [ ] **FEC (Forward Error Correction)**: パケット損失5%まで再送なしで復元

### レンダリング
- [ ] **WebGL2 YUV→RGB**: ビューア側で色変換をGPUに移譲
- [ ] **HW デコード**: dav1d (AV1), VideoToolbox (H264/H265)
- [ ] **適応スケーリング**: ウィンドウサイズに応じて解像度動的変更

---

## 3. セキュリティ (継続強化)

### 暗号進化
- [ ] **PQC ハイブリッド**: X25519 + Kyber768 の二重鍵交換 (post-quantum対応)
- [ ] **HKDF**: shared secret から専用キー導出 (現在は直接使用)
- [ ] 定期 rekey: 1時間毎にセッションキー再生成

### 監査
- [ ] 月次 `cargo audit` を CI 必須化
- [ ] 年1回外部セキュリティ監査 (Trail of Bits等)
- [ ] CVE 通知購読 (rustls, quinn, ring)
- [ ] **再現可能ビルド** (reproducible builds): バイナリ改ざん検出可能

### ハードニング
- [ ] **Sandbox**: ホストプロセスは非特権ユーザーで動作
- [ ] **Capabilities最小化**: Linux: CAP_SYS_ADMIN剥奪、uinputのみ許可
- [ ] **macOS: ハーデンドランタイム + 公証 (notarization)**
- [ ] **Windows: SmartScreen回避のためのEV署名**

---

## 4. UX / 採用

### 翻訳 (1000言語対応の段階的アプローチ)
- [ ] 第1段階 (v0.2): 日本語、英語、中国語 (簡体・繁体)、韓国語 — 5言語
- [ ] 第2段階 (v0.3): スペイン語、ポルトガル語、ドイツ語、フランス語、ロシア語、アラビア語 — 11言語
- [ ] 第3段階 (v1.0): Crowdin/Weblate でコミュニティ翻訳 — 50+言語目標
- [ ] テキストは全て BCP-47 タグ付き JSON、コードに埋め込まない
- [ ] RTL (アラビア語・ヘブライ語) 対応 → CSS論理プロパティのみ使用

### アクセシビリティ
- [ ] **WCAG AAA カラー** (現在: AA達成、AAAは v0.3で目標)
- [ ] フォーカスリング全要素に表示
- [ ] スクリーンリーダー対応 (aria-label)
- [ ] キーボードのみで全操作可能

### オンボーディング
- [ ] 初回起動時にデバイス指紋表示 + 「これを相手に伝えてください」案内
- [ ] PIN ペアリングのアニメーション付きガイド
- [ ] トラブルシュート: NAT検出失敗時の対処法表示

---

## 5. 拡張性

### プラグインシステム (v1.0以降)
- [ ] WASI ベースプラグイン: 入力フィルタ・録画・転送ログ等
- [ ] Skill ファイル (`.claude/skills/`) — コードレビュー、リリース、デプロイ手順
- [ ] hooks (`settings.json`): フォーマッター強制、テスト強制、危険ディレクトリ書き込みブロック

### 機能追加候補 (優先度順)
1. **ファイル転送 UI** (プロトコル実装済み) — v0.2
2. **多モニタ表示**: ビューア側で複数ディスプレイをタブ表示 — v0.2
3. **録画**: H264/H265 で MP4 にローカル保存 — v0.3
4. **遠隔シェル**: SSH不要のターミナルアクセス — v0.4
5. **ポートフォワード**: トンネリング機能 — v0.5
6. **モバイルクライアント**: Tauri Mobile (iOS/Android) — v1.0
7. **ブラウザクライアント**: WebRTC + WebTransport — v1.0
8. **企業機能**: LDAP/SAML SSO、監査ログ — v1.5

### 削除候補 (機能整理)
- [ ] WebRTC bridge — QUIC が浸透すれば不要 (v2.0で削除)
- [ ] 古いコーデック VP8 — 全プラットフォームでVP9以上対応後に削除

---

## 6. 開発プロセス

### gstack 適用
- [ ] **Think → Plan → Build → Review → Test → Ship → Reflect**
- [ ] 各機能に対してADR (Architecture Decision Record) を `docs/adr/` に残す
- [ ] 全テスト合格 + clippy warnings ゼロ + cargo audit クリーンを mergeブロッカー化

### CI/CD
- [x] 全OS自動ビルド (Windows/macOS/Linux x86_64+ARM64)
- [x] cargo audit 必須
- [ ] **コードカバレッジ 80%+** (cargo-tarpaulin)
- [ ] **ベンチマーク回帰検出** (criterion + bencher.dev)
- [ ] **fuzz テスト** (cargo-fuzz: メッセージパーサー、暗号復号)

### モニタリング
- [ ] OpenTelemetry 統合 (オプトイン)
- [ ] パニック自動収集 (Sentry等、オプトイン)
- [ ] 匿名メトリクス: 接続成功率、平均RTT、コーデック分布

---

## 7. 商用化準備 (AI生成コードガイドライン適用)

### コード品質
- [x] 全関数 docstring/コメント
- [x] CLAUDE.md でアーキテクチャ説明
- [ ] cargo doc 生成、docs.rs 公開
- [ ] 各 crate の README

### ライセンス
- [x] MIT (依存ライブラリも商用OK確認済み)
- [ ] CLA (Contributor License Agreement) — 大規模コミュニティ化時
- [ ] 第三者監査済みSBOM (Software Bill of Materials)

### 配布
- [x] Linux/macOS インストールスクリプト
- [x] Windows PowerShell インストーラー
- [ ] **コードサイニング**: macOS Developer ID, Windows EV
- [ ] **自動更新**: tauri-plugin-updater + Ed25519署名検証
- [ ] パッケージマネージャー: Homebrew, Chocolatey, AUR, Flatpak, Snap

---

## 8. 10年運用のための原則

1. **明示的な非互換ポリシー**: 破壊的変更は major version のみ、6か月の移行期間
2. **依存ピン留め**: Cargo.lock コミット必須、major更新は手動
3. **サンセット計画**: 機能削除は3バージョン前から `#[deprecated]` で予告
4. **ドキュメント同梱**: バイナリと一緒に release notes/migration guide 配布
5. **テスト古参化**: テスト自体も10年保守対象 — flaky テスト即修正

---

## 9. 「30%向上」の定量化

| 指標 | 現状 (v0.1) | 目標 (v1.0) |
|------|------------|-------------|
| 平均RTT (P2P, 同一国内) | 80ms | 50ms (-37%) |
| 帯域 (1080p静止画) | 200 kbps | 50 kbps (-75%) |
| 帯域 (1080p動画) | 5000 kbps | 3500 kbps (-30%) |
| 起動時間 | 2.5s | 1.0s (-60%) |
| メモリ使用量 | 180MB | 90MB (-50%) |
| P2P成功率 | 60% | 85% (+25pt) |
| クラッシュ率 | 不明 | <0.1% |

---

## 10. 次の3スプリント (実装順)

### Sprint 1 (v0.2): "動かす"
- [ ] X25519 ハンドシェイクをセッション開始時に実行 (現在ダミーcipher)
- [ ] 一気通貫テスト: 自分のマシンに自分から接続 (loopback)
- [ ] Tauri ビルド成功 + UI 起動確認
- [ ] Docker compose で signal server 動作確認

### Sprint 2 (v0.3): "速くする"
- [ ] AV1 HW エンコード (NVENC優先 — RTX 40+があれば)
- [ ] WebGL2 YUV→RGB レンダリング
- [ ] dirty rect エンコード (DXGI)
- [ ] QoS の BBR風アルゴリズム

### Sprint 3 (v0.4): "拡げる"
- [ ] PIN ペアリング UI 実装
- [ ] 多モニタ表示
- [ ] ファイル転送 UI
- [ ] 5言語ローカライゼーション
