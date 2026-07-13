# 外部動向調査ノート (2026-07)

Miru の残課題(`docs/FEATURE_AUDIT.md` / `docs/roadmap.md` セクション0.5)に
直接関係する外部の最新動向を Web 調査した結果。各項目は「現状 → Miru への含意
→ 出典」の順。**出典 URL は調査時点で実在確認済みだが、内容は要約であり、
実装前に一次情報を必ず再確認すること。**

## 1. コーデック動向 — AV1 は「最優先」でよいか

**現状 (2026)**: AV1 の HW エンコーダは成熟し(デバイス対応 ~88%、Netflix は
配信の30%が AV1)、しかしリアルタイム(サブ秒)用途では依然エンコード負荷が高く、
低遅延配信の現実解は H.265 とする評価が主流。AV1 が支配的コーデックになるのは
2028年頃との見立て。一方、AV1 の SCC (Screen Content Coding) ツール群
(palette mode / intra block copy)は、テキスト・ウィンドウ・シャープエッジという
画面共有特有のコンテンツに対して極めて有効。

**Miru への含意**:
- `miru-common/src/codec.rs` の `CODEC_PRIORITY` は AV1 を最優先にしているが、
  「AV1 HW エンコーダが実在する場合のみ AV1、無ければ H.265/VP9」という
  能力ベースの分岐が2026年時点では正しい。ソフトウェア AV1 リアルタイム
  エンコードには手を出さない(roadmap の AV1 HW 前提は妥当)
- VP9(現在の実コーデック)には screen-content 向けのチューニングがあり、
  `vpx.rs` の encoder 初期化で `VP9E_SET_TUNE_CONTENT=1` (VPX_CONTENT_SCREEN)
  を設定するのは低コストで効く改善 → **実施済み(要ビルド検証)**
- 将来 AV1 を配線する際は SCC ツールの有効化を必須要件とすること

**出典**:
- https://trembit.com/blog/av1-in-2026-why-the-next-gen-codec-still-isnt-dominant/
- https://visionular.ai/av1-screen-content-coding/
- https://www.red5.net/blog/av1-vs-h265/
- https://www.forasoft.com/blog/article/av1-codec-production-streaming-2026

## 2. Wayland / PipeWire キャプチャ — 先行者の失敗から学ぶ

**現状 (2026)**: xdg-desktop-portal (org.freedesktop.portal.ScreenCast) +
PipeWire がWaylandキャプチャの唯一の正道。DMA-BUF (DRM PRIME fd) でゼロコピー
可能だが、コンポジタごとの形式ネゴシエーションに実装差があり、RustDesk は
COSMIC / DMA-BUF portal で「caps が BGRx システムメモリのみ受理」問題を踏んで
videoconvert 挿入で修正した実績がある(PR #15063)。portal 不在時のエラー
メッセージ改善(PR #14543)や、portal の同意ダイアログを回避する DRM/KMS 直接
キャプチャの議論(#15417、ログイン画面対応)も進行中。Rust のモジュール化された
参照実装として libscreencapture-wayland、lamco-wayland がある。

**Miru への含意**:
- `crates/miru-capture/src/platform/pipewire.rs` を実装する際、RustDesk の
  該当 PR / issue を先に読むことで同じ罠(DMA-BUF caps ネゴ失敗、COSMIC 対応、
  portal 不在時の不親切なエラー)を回避できる
- 最初のマイルストーンは「DMA-BUF ゼロコピー」ではなく「メモリマップ経由で
  まず動く」に置き、ゼロコピーは第2段階にする(RustDesk も videoconvert
  フォールバックを持つ)
- 無人アクセス(ログイン画面)は portal では原理的に不可。将来要件になったら
  DRM/KMS 直接キャプチャ(要 root/専用権限)を別バックエンドとして検討

**出典**:
- https://github.com/rustdesk/rustdesk/pull/15063
- https://github.com/rustdesk/rustdesk/pull/14543
- https://github.com/rustdesk/rustdesk/discussions/15417
- https://github.com/DafabHoid/libscreencapture-wayland
- https://github.com/lamco-admin/lamco-wayland

## 3. Media over QUIC (MoQ) — QUIC 優先パスの設計先

**現状 (2026)**: IETF MoQ WG の transport draft (draft-ietf-moq-transport) は
WG Last Call に向かって成熟中。リモートデスクトップ・クラウドゲーミングが明示的な
ターゲット用途に含まれる。Cloudflare が330都市以上に MoQ リレーを展開、NAB 2026
で11ベンダーが相互運用デモ、サブ500ms 配信の実証済み。pub/sub モデルで
ストリーム/データグラム/優先度/部分信頼性を活用し、中継リレー経由でもスケールする。

**Miru への含意**:
- roadmap の「QUIC 優先パス」「多ストリーム並列(HOL-blocking 回避)」「FEC」は、
  独自プロトコルを発明せず MoQT の設計(またはサブセット)に寄せるのが
  10年運用の観点で有利(IETF 標準 + 実装群が既に存在)
- 現行の WebSocket リレーは動いているので置き換えは急がない。`miru-transport`
  の QUIC 化착手時に draft-ietf-moq-transport を必読文献とする

**出典**:
- https://datatracker.ietf.org/doc/draft-ietf-moq-transport/
- https://www.ietf.org/blog/moq-overview/
- https://www.forasoft.com/learn/video-streaming/articles-streaming/media-over-quic-moq
- https://arxiv.org/pdf/2310.03256 (Toward One-Second Latency: Evolution of Live Media Streaming)

## 4. ポスト量子暗号 — 「Kyber768」は名称も状況も更新が必要

**現状 (2026)**: NIST 標準化後の正式名は **ML-KEM-768**(Kyber768 は旧称)。
X25519+ML-KEM-768 ハイブリッド(TLS group `X25519MLKEM768`, codepoint 0x11EC)が
Chrome / Edge / Firefox でデフォルト有効になり、Cloudflare・Google の
エッジも広告済み。2026年の「事実上の標準」。OpenSSL 3.5 が ML-KEM/ML-DSA/
SLH-DSA をネイティブサポート。正しい combiner は「両鍵交換を並列実行 →
共有秘密を連結 → ハンドシェイクトランスクリプトを salt に HKDF で導出」。

**Miru への含意**:
- `CLAUDE.md` の「将来: Kyber768 ハイブリッド」は ML-KEM-768 に用語更新すべき
  (本コミットで実施)
- 実装時は `miru-transport/src/handshake.rs` の X25519 DH の直後に ML-KEM
  encap/decap を並列追加し、`shared_x25519 || shared_mlkem` を HKDF に入れる
  形が業界標準の combiner と一致する。Rust には ml-kem crate (RustCrypto) が
  存在する
- 「harvest now, decrypt later」対策として、E2E を売りにする Miru では
  優先度を上げる価値がある(競合との差別化点にもなる)

**出典**:
- https://www.postquantumsecurity.org/publications/X25519+MLKEM768.html
- https://quickztna.com/blog/hybrid-key-exchange-x25519-mlkem/
- https://www.systemshardening.com/articles/network/tls-post-quantum-hybrid-deployment/
- https://arxiv.org/pdf/2606.16473 (Measurement Study of Post-Quantum Readiness: 2026)

## 5. システム音声キャプチャ — 実装手段の具体化 (FEATURE_AUDIT 🔴#1)

**現状 (2026)**:
- **macOS**: cpal (RustAudio) が macOS 14.6+ の CoreAudio loopback recording を
  サポート済み — 追加のネイティブコードなしで取得できる可能性が高い
- **Windows**: WASAPI loopback は `AUDCLNT_STREAMFLAGS_LOOPBACK` で取得。
  ただし **`AUDCLNT_STREAMFLAGS_EVENTCALLBACK` と併用できない**という既知の罠
  があり(cpal issue #251)、ポーリング型の取得ループが必要。安全ラッパーとして
  専用の `wasapi` crate も存在
- **Linux**: PulseAudio の monitor デバイス(各出力 sink に対応する
  `<sink>.monitor` ソース)から capture する方式が確立している。PipeWire 環境でも
  PulseAudio 互換層経由で同じ方式が機能する

**Miru への含意**:
- `miru-audio` の Opus encoder は完成済みなので、必要なのは「OS 音声 →
  f32 interleaved サンプル列」を返すキャプチャ層のみ
- cpal を第一候補にすると3プラットフォームを1 API で覆える可能性がある
  (macOS loopback は cpal 対応済み、Linux は monitor ソースをデバイスとして
  列挙可能、Windows loopback のみ cpal 未対応の場合は `wasapi` crate で補完)
- 実装時は WASAPI の EVENTCALLBACK 非互換の罠を設計段階から織り込むこと

**出典**:
- https://github.com/RustAudio/cpal (README / releases)
- https://github.com/tomaka/cpal/issues/251
- https://docs.rs/wasapi
- https://deepwiki.com/rustdesk/rustdesk/5.3-audio-service

## 6. RustDesk 2026 CVE 動向 — 同型アーキテクチャからの教訓(最重要)

**現状 (2026-03 公開)**: RustDesk に複数の重大 CVE。Miru と**同じ signal/relay
アーキテクチャ**を持つため直接照合した:

- **CVE-2026-30784** (Critical): rendezvous (hbbs) の `handle_punch_hole_request()`
  と **RegisterPeer ハンドラが認証なしで悪意あるピアを登録可能** → 権限昇格。
  relay forwarding にもアクセス制御なし。
  **Miru の状況**: `miru-signal` の Register ハンドラは既に
  `verify_register_signature` (Ed25519 所有権証明) + identity-lock を実装済みで
  この攻撃面をほぼ塞いでいる(コード実査で確認)。**唯一の残存緩和点**は、
  後方互換のため「署名なし Register を警告付きで受理」している点
  (`main.rs` の `Ok(false) => warn!(...)` 分岐)。全クライアント移行後に
  署名必須化(`Ok(false)` を reject に)すれば完全に閉じる。
- **CVE-2026-30794** (AiTM): TLS retry が `danger_accept_invalid_certs(true)` を
  使用。**Miru**: 該当関数を一切提供しない設計(TRUST.md 既載)。
- **CVE-2026-30795**: Heartbeat 同期ループの平文送信。**Miru**: 全セッション
  メッセージが ChaCha20-Poly1305 暗号化、signal 経由は device_id/pubkey のみ。
- **CVE-2026-30792** (MitM): strategy 同期/HTTP API のメッセージ改ざん。
  **Miru**: HTTP API 設定同期チャネル自体が存在しない(該当せず)。

**Miru への含意**: 本調査で判明した **具体的な次アクションは1つ** — 全クライアント
が署名付き Register に移行し次第、`miru-signal` の未署名 Register 受理を
reject に切り替える(CVE-2026-30784 の残存緩和点を閉じる)。→ **実装済み(要ビルド検証)**:
`MIRU_REQUIRE_SIGNED_REGISTER=1` を設定すると未署名 Register を 403 で拒否する
opt-in strict モードを追加(`MIRU_ALLOW_STUB`/`MIRU_REQUIRE_PAIRING_CONFIRM` と
同じ「デフォルト維持・明示的 opt-in」パターン)。デフォルトは後方互換のため
従来どおり警告付き受理。それ以外の RustDesk 2026 CVE は Miru の既存設計で
構造的にカバー済み。

**出典**:
- https://www.sentinelone.com/vulnerability-database/cve-2026-30784/
- https://vulnerability.circl.lu/vuln/cve-2026-30784
- https://www.sentinelone.com/vulnerability-database/cve-2026-30794/
- https://www.sentinelone.com/vulnerability-database/cve-2026-30795/
- https://app.opencve.io/cve/?vendor=rustdesk

## 7. macOS ScreenCaptureKit — CGDisplay は積極的 deprecation フェーズへ

**現状 (2026)**: 旧 Quartz CGDisplay API は deprecated。**macOS Sequoia 15.1
以降、CGScreenCapture 系を使うアプリに対し OS が「セキュリティ設定を回避しようと
している」旨のユーザー警告を出し始めた**。ScreenCaptureKit (SCStream) が正道。
Rust バインディングは `screencapturekit` crate (crates.io) が存在。SCStream の
停止通知は `SCStreamDelegateTrait::did_stop_with_error` に一本化(旧
`stream_did_stop` は deprecated)。`CGPreflightScreenCaptureAccess` も 15.1 で
deprecation issue が上がっている(xcap#160)。

**Miru への含意**:
- FEATURE_AUDIT 🔴#5 (macOS 20fps) は「性能改善」だけでなく **2026年時点では
  互換性リスク**になりつつある — Sequoia でユーザーに警告が出るなら UX 上の
  ブロッカーに近い。ScreenCaptureKit 移行の優先度を上げる材料
- Rust から `screencapturekit` crate を使えば FFI を自前で書かずに済む可能性

**出典**:
- https://developer.apple.com/documentation/screencapturekit/
- https://crates.io/crates/screencapturekit
- https://github.com/nashaofu/xcap/issues/160
- https://github.com/svtlabs/screencapturekit-rs

## 8. Tauri v2 配布パイプライン — 署名/公証/updater の実務

**現状 (2026)**: Tauri v2 は署名・公証・自動更新の公式ドキュメントが整備済み:
- **macOS**: Developer ID Application 証明書で署名 → Tauri がビルド時に **公証を
  自動実行**(App Store Connect API または Apple ID の認証情報を環境変数で渡す)。
  **無料 Apple Developer アカウントでは公証不可**(有料 $99/年が必須)
- **Windows**: OV 証明書 + Azure Key Vault 方式が推奨。署名なしは SmartScreen 警告
- **updater**: `npm run tauri signer generate` で鍵ペア生成、
  `TAURI_SIGNING_PRIVATE_KEY` 等の環境変数で署名。`latest.json` を配布

**Miru への含意**:
- roadmap §0.5 フェーズ2(署名・自動更新)の実装手順が具体化。Tauri v2 の
  updater は Ed25519 署名 + `latest.json` で、`docs/updater.md` の既存設計と
  一致する(公式プラグインをそのまま使える)
- **コスト要件が判明**: macOS 配布には有料 Apple Developer Program ($99/年) が
  必須。「完全無料でセルフホスト」を掲げる Miru でも、署名済み macOS バイナリを
  配るには開発元の有料アカウントが要る点を README/roadmap に明記すべき

**出典**:
- https://v2.tauri.app/distribute/sign/macos/
- https://v2.tauri.app/distribute/sign/windows/
- https://v2.tauri.app/plugin/updater/
- https://dev.to/tomtomdu73/ship-your-tauri-v2-app-like-a-pro-code-signing-for-macos-and-windows-part-12-3o9n

## 調査メタ情報

- 調査日: 2026-07 / 方法: Web 検索(第1回4クエリ + 第2回4クエリ)
- 本調査で判明した唯一の具体コードアクション: §6 の CVE-2026-30784 残存緩和点
  (未署名 Register 受理)の将来クローズ。それ以外は設計判断/優先度材料。
- 残テーマ: WebRTC DataChannel vs raw QUIC のベンチ比較、Opus の DTX/FEC 設定
