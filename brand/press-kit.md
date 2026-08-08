# Miru プレスキット

メディア・ブログ・レビュー記事執筆者向けの資料集。

## 一行紹介

> Miru は、AI 時代の安全な PC fabric。リモートデスクトップ、AI エージェント基盤、
> 検証可能な E2E、多デバイス constellation を統合した MIT ライセンスの OSS。

## 30秒紹介

Miru (見る) は、リモートデスクトップを「人間 ↔ 人間の画面共有」から「全デバイスと
全エージェントが安全に協調する fabric」へ拡張した OSS です。

競合がやっていない4軸を設計の初日から組み込んでいます:

1. **AI Agent First Class** — Claude/GPT 等が capability-scoped で PC 制御
2. **Verifiable E2E** — 「E2E です」を主張ではなく数学的に証明可能
3. **Device Constellation** — 多デバイスを N:N の fabric として扱う
4. **Temporal** — Premiere風 scrubber でセッションを巻き戻し再生

MIT ライセンス、ゼロアカウント、セルフホスト可能、テレメトリ一切なし。

## 2分紹介

リモートデスクトップは20年間ほぼ変わっていません。RustDesk、AnyDesk、Parsec、
TeamViewer — いずれも「人間が遠隔から PC を見て操作する」という前提です。
だが世界は変わりました:

**AI エージェント時代**: Claude Computer Use、OpenAI Operator、Adept ACT-1 等、
AI が PC 操作する製品が急増しています。各社が独自に画面キャプチャ + 入力注入を
再実装しているのが現状で、汎用かつ安全な基盤が欠落しています。

**多デバイス時代**: ノート/スマホ/デスクトップ/サーバーを束ねる fabric がない。

**検証可能セキュリティの時代**: 「E2E です」と主張するだけでは足りない。

Miru はこれらに正面から取り組みます:

- **`miru-mcp`** バイナリは Model Context Protocol 経由で Claude/任意の LLM から
  Miru ホストを操作できます。各操作は capability-scoped Ed25519 トークンで認可され、
  改ざん検知付きの audit log (SHA-256 chain) に永続記録されます。
- **`miru-transparency`** クレートは各セッションのメタデータを暗号学的にコミット
  します。両端の Ed25519 共同署名 + Sigstore Rekor の透明性ログにより、6か月後でも
  「あのセッションが改ざんされていない」を誰でも数学的に検証できます。
- **`miru-constellation`** クレートは利用者の全デバイスを Ed25519 署名グラフで
  束ね、capability 広告 (解像度・mic・battery・status) と handoff token による
  シームレスな移譲を実現します。
- **時間軸スクラバー** はセッションを deterministic に記録し、Premiere風 UI で
  巻き戻し・倍速・ブックマークが可能。監査・教育・デバッグの4D UI です。

技術スタックは Rust (16クレート、約11,000行) + Tauri/React (約1,700行)。
セキュリティは X25519 + ChaCha20-Poly1305 + Ed25519 + Argon2id を採用、すべて
監査済みクレート、自前暗号実装ゼロです。

RustDesk の最近の CVE (CVE-2026-30785/30794/2490 等) を教訓に、Argon2id 採用、
TLS 検証無効フォールバック構造的禁止、O_NOFOLLOW + path 検証で同種の脆弱性を
構造的に回避しています。

## キーメッセージ

| メッセージ | 詳細 |
|-----------|------|
| **AI 時代向けに最初から設計** | Anthropic Computer Use 等の基盤候補 |
| **検証可能 E2E の OSS** | Sigstore Rekor 互換、6か月後でも証明可能 |
| **多デバイス fabric** | N:N 接続、Apple Continuity をクロス OS で |
| **テレメトリ一切なし** | プライバシー要件のある業界に最適 |
| **MIT ライセンス** | RustDesk の AGPLv3 より商用統合容易 |
| **日本発 OSS** | 個人開発者による設計、日本語第一 |
| **再現可能ビルド** | 同じコミットから誰でも bit-identical なバイナリ |

## ビジュアル

(v0.1 リリース時に画像同梱予定)

| アセット | 用途 | フォーマット |
|---------|------|-------------|
| `logo.svg` (square) | アイコン、favicon | SVG, 256×256 PNG |
| `wordmark.svg` (horizontal) | ヘッダー、記事冒頭 | SVG, 横長 |
| Constellation map スクリーンショット | 機能紹介 | PNG, 1920×1200 |
| Audit Log スクリーンショット | セキュリティ訴求 | PNG, 1920×1200 |
| Timeline scrubber スクリーンショット | 4D UI 訴求 | PNG, 1920×1200 |
| AI token issue ダイアログ | AI 統合訴求 | PNG, 1200×900 |
| Onboarding wizard | UX 訴求 | PNG, 1200×900 |

ダウンロード: `https://github.com/shizukutanaka/miru/tree/main/brand/press/` (v0.1 同梱)

## 数字

| 項目 | 値 |
|------|---|
| ソースコード規模 | 約 11,000 行 Rust + 約 1,700 行 TS |
| Rust crate 数 | 16 |
| サポート OS | Windows 10+ / macOS 12+ / Linux (X11/Wayland) |
| 計画サポート言語数 | 1000+ |
| ライセンス | MIT |
| 最低 Rust バージョン | 1.80+ |
| バイナリサイズ目標 | 10-15 MB |
| メモリ常駐目標 | 50 MB |

## 比較表 (簡易)

| 機能 | Miru | RustDesk | Parsec | AnyDesk | TeamViewer |
|------|------|----------|--------|---------|------------|
| オープンソース | MIT | AGPLv3 | × | × | × |
| 無料商用利用 | ✓ | ✓ | △ | $588/年〜 | $49/月〜 |
| AI エージェント基盤 | ✓ | × | × | × | × |
| 検証可能 E2E | ✓ | × | × | × | × |
| 多デバイス constellation | ✓ | × | × | × | × |
| セッション scrubber | ✓ | × | × | △ | △ |
| HDR | ✓ | △ | × | × | × |
| 5.1/7.1 サラウンド | ✓ | × | × | × | × |
| テレメトリなし | ✓ | △ | × | × | × |

詳細: [`docs/competitive-analysis.md`](../docs/competitive-analysis.md)

## 想定される質問

### Q: なぜ「Miru」?

A: 日本語の「見る」(to see)。リモートデスクトップは「相手の画面を見る」ことが
本質という考えから。

### Q: 個人プロジェクト?

A: はい。shizukutanaka 個人のプロジェクト。

### Q: ビジネスモデルは?

A: コアは MIT で永久無料。将来的にエンタープライズサポート (audit support、
オンサイト導入支援) を有償提供予定。コードを売るのではなく、サポートを提供する形。

### Q: 競合 (RustDesk 等) より優れていますか?

A: 機能数では劣る部分があります (Web/モバイルクライアント等)。差別化は「他にない
4軸」の戦略的位置取りです。RustDesk が AGPL で提供している基本機能を Miru で
全部置き換えることは v0.1 の目標ではありません。

### Q: Anthropic / OpenAI と関係はありますか?

A: ありません。Miru は Anthropic Computer Use や OpenAI Operator の代替・補完
ツールとしての位置付けですが、これらの企業からの後援や提携はありません。

### Q: 日本語以外でも使えますか?

A: 1000+ 言語対応を目標としています。現在は JA + EN。EN を fallback に各国語の
PR を歓迎しています (locales/README.md 参照)。

## 連絡先

| 用途 | 連絡先 |
|------|-------|
| 一般的な質問 | GitHub Discussions |
| プレス/メディア | 専用アドレス未設置(GitHub Issues 経由。`press@miru.app` はまだ稼働していません) |
| パートナーシップ | 専用アドレス未設置(GitHub Issues 経由。`partnerships@miru.app` はまだ稼働していません) |
| セキュリティ | [SECURITY.md](../SECURITY.md) 参照(`security@miru.app` はまだ稼働していません) |
| 法務 | 専用アドレス未設置(GitHub Issues 経由。`legal@miru.app` はまだ稼働していません) |

## 引用 OK 文章

「Miru は、AI 時代の安全な PC fabric。リモートデスクトップ、AI エージェント基盤、
検証可能な E2E、多デバイス constellation を統合した MIT ライセンスの OSS。」

「Miru は、競合がやっていない4軸 (AI Agent First Class / Verifiable E2E /
Device Constellation / Temporal) を設計の初日から組み込んでいます。」

「Miru は RustDesk の最近の CVE を教訓に、Argon2id 採用、TLS 検証無効
フォールバック構造的禁止、O_NOFOLLOW + path 検証で同種の脆弱性を構造的に回避
しています。」
