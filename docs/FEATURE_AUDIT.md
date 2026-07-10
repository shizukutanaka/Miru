# Miru 機能過不足監査 (FEATURE_AUDIT)

作成: 2026-07 / ブランチ: `claude/sweet-franklin-l5a1yw`

## このドキュメントの目的と読み方

これは **前提知識ゼロの引き継ぎ先(人間・LLM いずれも)に向けたハンドオフ文書** です。
Miru は「TeamViewer/AnyDesk 代替。完全セルフホスト可能、E2E 暗号化、P2P 優先」を
目的とする Rust + Tauri v2 製リモートデスクトップです(ルートの `CLAUDE.md` 参照)。

本文書は全 crate のコードを実際に読んで機能の「不足・過剰・適正」を選別した結果です。
**各項目には根拠となるファイルパスを付してあるので、鵜呑みにせず該当ファイルを
開いて現状を再確認してから作業してください**(本文書作成後に実装が進んでいる
可能性があるため)。関連文書: `docs/PRODUCT_REVIEW.md`(実ビルド検証付きレビュー)、
`docs/roadmap.md` セクション 0(優先順位の意思決定記録)・セクション 0.5
(市販レベル品質へのロードマップ — コード署名/自動更新/クラッシュレポート等、
本文書がカバーしないコア機能以外の商用リリース要件)。

## 判定基準(1文)

> 「今日この製品を人に配って、音声付き 60fps HW エンコード映像が初回接続で
> 見られるか」に寄与しない実装は v0.1 では凍結する。

この基準に照らして全機能を 4 区分に選別した。

---

## 🔴 不足 — コア目的に必須なのに欠けている(優先実装対象)

| # | 機能 | 現状の証拠(ファイルパス) | 対応方針 / 規模 |
|---|------|---------------------------|------------------|
| 1 | ホスト側システム音声キャプチャ | `crates/miru-host/src/` 内のどこにも `AudioEncoder` の呼び出しが無く、`AudioFrame` が一度も送信されない。Opus エンコード/デコード/再生 (`crates/miru-audio/src/lib.rs`) とコーデック交渉 (`crates/miru-common/src/codec.rs` の `negotiate_audio`) は完成済みだが宙に浮いている。`crates/miru-host/src/session.rs` の `Features.audio` は虚偽広告を避けるため `false` に修正済み | WASAPI loopback (Windows) / PulseAudio monitor (Linux) / CoreAudio tap (macOS) の新規実装。`cpal` 等の依存追加を検討。規模: 中 |
| 2 | HW エンコード (AV1/H264/H265) | `crates/miru-codec/src/ffmpeg_enc.rs` — `encode()` が `bail!("FFmpegEncoder not yet wired")` を返すだけのスタブ。実コーデックは JPEG と VP9 (feature `vpx`) のみ | ffmpeg-next の実配線。ソース内 TODO コメントに実装手順の骨子あり。規模: 中 (2-3週) |
| 3 | WebGL2 YUV 描画パス | シェーダーは準備済みだが本番導線は JPEG プレビューのみ (`crates/miru-client/ui/src/` 配下)。CLAUDE.md の残作業リストにも記載 | YUV フレームを実通信フローに接続。規模: 小〜中 |
| 4 | Wayland / PipeWire キャプチャ | `crates/miru-capture/src/platform/pipewire.rs` がスタブ。Ubuntu 22.04+ / Fedora の標準セッション(Wayland)で動かない | xdg-desktop-portal (zbus) + pipewire-rs。規模: 中 (2-3週) |
| 5 | macOS 60fps キャプチャ | `crates/miru-capture/src/platform/macos.rs` — deprecated な CGDisplayCreateImage で ~20fps | ScreenCaptureKit (SCStream) へ移行。規模: 中 (1-2週) |
| 6 | CI/CD が稼働していない | ワークフローが `.github/workflows/` ではなく `.github/workflows-proposed/` に隔離されたまま。push する GitHub App に `workflows` 権限が無く自動化エージェントでは移動できない(実際に push が拒否されることを確認済み) | **リポジトリ管理者の手作業が必要**: `git mv .github/workflows-proposed/*.yml .github/workflows/`。規模: 極小 |
| 7 | ~~クライアント側 TOFU 確認ダイアログが完全に未接続~~ → **実装済み(要 cargo test 検証)** | `crates/miru-client/src-tauri/src/session.rs` のハンドシェイク直後に TOFU ゲートを追加: viewer 側 `AclStore`(`crates/miru-auth/src/lib.rs`)の `check()`/`trust()`/`touch()` を初めて呼び出すよう配線。`TrustDecision::Unknown`(初回接続)は `pending_pairing` oneshot チャネル経由でフロントエンドの確認を待ち(120秒タイムアウト)、`PairingDialog.tsx`(`requirePin={false}` モード)で指紋を能動的に確認させてから `TrustedPeer` を永続化する。`TrustDecision::PubkeyMismatch`(鍵変更 = MITM の可能性)は `NoAutoRetry` マーカーエラーで即座に拒否し、再接続ループが自動リトライしないようにも対応済み。新規 Tauri command `confirm_pairing`。TypeScript 側は `npx tsc -b --noEmit` でエラーゼロを実機検証済み。**Rust 側はサンドボックスの crates.io 遮断によりコンパイル未検証** — マージ前に必ず `cargo test --workspace` を実行すること | 完了。次のアクションは Rust ビルド検証のみ |
| 8 | ~~ホスト側 TOFU が無条件自動承認~~ → **オプトイン強化モードを追加(要 cargo test 検証)** | `crates/miru-host/src/session.rs` の `check_or_pair()`。`miru-host` は GUI の無い純粋な CLI/ヘッドレスバイナリのため viewer 側と同じ対話的確認ダイアログは実装できない。デフォルト挙動(初回接続は自動承認)を無断で変える判断は破壊的変更のリスクがあるため、**デフォルトは変更せず** `HostConfig::require_pairing_confirm`(`MIRU_REQUIRE_PAIRING_CONFIRM=1` で有効化、`main.rs` で一度だけ読み取り)を追加。`MIRU_ALLOW_STUB` の「デフォルト維持・明示的 opt-in」パターンを踏襲 | 完了。次のアクションは Rust ビルド検証のみ |
| 9 | `ConnectArgs.pin` フィールドが実質デッドコード | `ConnectScreen.tsx` の「PIN(初回のみ)」入力欄が `session.rs`/`commands.rs` のどこからも参照されない(未検証)。`CLAUDE.md` は PBKDF2-SHA256 ペアリングを設計として明記 | 実装するか誤解を招く入力欄を外すか要相談。**未着手**。規模: 小〜中 |
| 10 | UI アクセシビリティ欠如 | 商用品質監査(2026-07)で確認: 12個の `.tsx` 全体で `aria-*`/`role=` 使用が実質ゼロだった。`ConnectScreen.tsx`(label/input の `id`/`htmlFor` 関連付け、`role="alert"`、ピア接続ボタンの `aria-label`)と `PairingDialog.tsx`(`role="dialog"` + `aria-modal`/`aria-labelledby`)は対応済み(`tsc` 検証済み) | 残り10コンポーネント(`SessionScreen`/`OnboardingWizard`/`AuditLogViewer`/`ConstellationMap`/`AgentTokenIssue`/`TimelineScrubber`/`DisplayTabs`/`PairPrompt`/`App.tsx`)が未着手。規模: 小(コンポーネントごと) |
| 11 | ~~存在しない `miru.app` ドメインへの依存~~ → **解消済み** | `MIRU_SIGNAL` のデフォルト値が未登録の `signal.miru.app` を指していた(`main.rs`/`scripts/install.sh` 双方)。`PRIVACY.md` は実装ゼロのクラッシュレポート機能を実在するかのように詳述していた(`--enable-crash-reports` フラグはコード上皆無、`grep` で確認済み)。全て `localhost` デフォルトへの修正、または「未実装/未稼働」の正直な注記に置換 | 完了 |

## 🟡 過剰 — 品質は高いがコア未完成の段階では時期尚早(追加投資を凍結)

| # | 機能 | 証拠 | 判断理由 |
|---|------|------|---------|
| 1 | AI エージェント統治基盤 6 crate | `crates/miru-agent/` (11 capability × 3 セキュリティ層のトークン)、`crates/miru-mcp/` (MCP サーバ)、`crates/miru-sandbox/`、`crates/miru-constellation/`、`crates/miru-discovery/`、`crates/miru-transparency/` (Rekor 透明性ログ) | 「人間向けリモートデスクトップ」という一次ゴールの定義外。設計品質は業界水準以上だが、実利用データが無いまま拡張するべきではない。**削除はしない。凍結のみ** |
| 2 | セッション録画 | `crates/miru-host/src/recording.rs` + `crates/miru-client/ui/src/components/TimelineScrubber.tsx` | ライブ接続の基本体験(音声・HW エンコード)が未完成な段階での拡張機能 |
| 3 | NAT 越えのさらなる高度化 | `crates/miru-transport/src/nat.rs` — STUN + hole-punch + NAT タイプ判定は実装済み | 現状で十分。ソース内コメント通り Symmetric NAT/CGNAT は元々リレー行きであり、これ以上磨いても回収が薄い |
| 4 | AI エージェント系 UI のトップレベル露出 | `crates/miru-client/ui/src/components/` の `ConstellationMap.tsx` / `AuditLogViewer.tsx` / `AgentTokenIssue.tsx` — トップレベル 9 コンポーネント中 3 つが一般ユーザーに無関係 | 削除ではなく「詳細設定の奥へ再配置」または起動フラグの配下に隠す情報アーキテクチャ変更が correct fix |

## 🟢 適正 — 過不足なし(触らない)

| # | 機能 | 証拠 |
|---|------|------|
| 1 | 暗号層 | `crates/miru-common/src/crypto.rs` — X25519 ephemeral + ChaCha20-Poly1305、方向別サブ鍵、128-bit sliding window リプレイ保護。`crates/miru-transport/src/handshake.rs` — Ed25519 `verify_strict` + TOFU |
| 2 | QoS / バックプレッシャ | `crates/miru-host/src/qos_bbr.rs` (BBR 風制御) + `crates/miru-host/src/backpressure.rs`。RustDesk の AIMD より先進的 |
| 3 | i18n + オンボーディング | `crates/miru-client/ui/src/locales/{ja,en}.json`、`OnboardingWizard.tsx` |
| 4 | 配布基盤 | `scripts/install.sh` / `install.ps1`、`distribution/{aur,homebrew,scoop}/` |
| 5 | テスト文化 | 統合テスト・ベンチ (`crates/miru-bench/`)・ファズ (`fuzz/`)・リグレッションテストが機能追加に伴い整備されている |

## ⚠️ 要事実確認 — 過不足判定の前に確認が必要

1. **配布経路が既に一般公開されているか** — `distribution/` のパッケージ定義が
   AUR/Homebrew/Scoop に実際に登録済みなら、「音声も HW エンコードも動かない版」が
   ユーザーの手元に届いている実害リスクがある。公開状況を確認せよ。
2. **Merkle 木の RFC 6962 非準拠** — `crates/miru-transparency/` は odd leaf を
   self-hash する実装で、内部一貫性はあるが他実装と相互運用できない
   (`docs/PRODUCT_REVIEW.md` 参照)。外部相互運用が要件になるかで過剰/欠陥の判定が変わる。

---

## 本ブランチで適用済みの修正(引き継ぎ時の注意)

ブランチ `claude/sweet-franklin-l5a1yw` に20コミット以上を追加済み。主な内容
(詳細は `git log` を参照):

- perf: RGBA/BGRA→I420 変換の高速化(clone排除・固定小数点化)、後に
  `packed32_to_i420` へ重複排除して統合
- fix: relay接続タイムアウト、クリップボードサイズ上限、backpressureの
  atomic ordering、MCPレート制限の切り上げ除算、display index範囲チェック
- fix: `Features.audio` の虚偽広告修正
- feat: viewer側TOFU確認ゲート、host側 opt-in強化モード(`MIRU_REQUIRE_PAIRING_CONFIRM`)
- feat: UI アクセシビリティ基礎対応(2/12コンポーネント)
- fix: 存在しない `miru.app` ドメインへの依存を解消(コード・配布パッケージ・
  法的文書すべて)
- docs: `PRODUCT_REVIEW.md`/`roadmap.md`(優先順位分析 + 商用品質ロードマップ)/
  `MODEL_GUIDE.md`(モデル・skill・Agent・Loop使い分け)の新規作成・更新
- `/code-review`・`/simplify` スキルによるセルフレビューを実施済み(clippy警告
  修正、BGRA/RGBA変換の重複排除、env変数のconfig field化)

**重要**: これらのコード変更は目視レビューのみで **コンパイル未検証** です。
作成時のサンドボックスは crates.io (`static.crates.io`) への通信がプロキシポリシーで
403 拒否され `cargo check` が実行できませんでした。引き継いだら **まず
`cargo test --workspace` を実行して全変更をビルド・テスト検証してください**。
