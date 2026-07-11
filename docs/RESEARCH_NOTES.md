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

## 調査メタ情報

- 調査日: 2026-07 / 方法: Web 検索(4クエリ)
- 未調査で価値がありそうな残りテーマ: ScreenCaptureKit の最新 API 変更、
  Tauri v2 の updater/notarization 実務、RustDesk の 2026 年 CVE 動向
