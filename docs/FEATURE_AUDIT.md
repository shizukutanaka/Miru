# Miru 機能過不足監査 (FEATURE_AUDIT)

作成: 2026-07 / ブランチ: `claude/sweet-franklin-l5a1yw`

> **⚠️ 未検証 Rust で実際に見つかったコンパイルエラー: 2件**(いずれも本ブランチ内で
> 自分が入れたもの)。① `SessionEvent` にフィールドを追加した際、`state.rs` の構築
> リテラルが追随せず。② `session::run` に `audio_muted` 引数を追加した編集が失われ、
> 本体が未定義変数を参照 + 呼び出し側と引数の数が不一致。**どちらも目視レビューを
> すり抜け、機械的な照合で初めて出た**。`cargo test --workspace` が未実行である限り
> 同種が残っている前提で扱うこと(`scripts/verify-offline.sh` はクレート境界を
> またぐ型・アリティ不整合を検出できない)。
>
> **2026-08 更新(ADR 0022)**: スコープの矛盾を記録。ADR 0005(AI 軸で差別化)・
> roadmap §0(同じ6 crate を凍結)・本監査の出荷基準(パリティ)が**異なる「完成」を
> 定義**しており、凍結スコープが Rust 全体の **32%(7,173行)** を占めたままビルド・CI・
> 攻撃面・入力ホットパスに課金し続けている。ADR 0022 は差別化軸を捨てず feature gate で
> デフォルト off にし、出荷基準をパリティへ一本化することを提案(実行は要ビルド環境)。
> あわせて **`scripts/verify-offline.sh`** を追加 — crates.io 無しでも
> 全 `.rs` の構文チェック + std 依存モジュール(keymap 11件 / backoff 9件)の
> **実テスト実行** + フロント検証が回る。
>
> **2026-07 更新**: 本ブランチで項目 3(WebGL2→WebCodecs)・7・8(TOFU)・9(PIN デッド
> コード)・10(a11y)・11(miru.app)が解消済み。残タスク(音声キャプチャ・HW エンコード・
> Wayland・macOS・CI・cargo 検証)を **後続エージェントが追加調査なしで着手できる粒度**に
> 落とした実行指示書は **`docs/WORK_INSTRUCTIONS.md`** を参照。本監査は履歴として保持。

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
| 1 | ~~ホスト側システム音声キャプチャ~~ → **loopback/monitor デバイス経由で実装(要 cargo test + 実機検証)** | `crates/miru-audio/src/capture.rs`(新規 `SystemAudioCapture` = cpal 入力ストリーム、48kHz stereo f32 固定)+ `crates/miru-host/src/audio_loop.rs`(新規、`capture_loop.rs` と同型に専用スレッドで capture→Opus encode→`flume::Receiver<Msg>`)。`session.rs` は `Features.audio` を `loopback_available()` で広告、Opus 交渉時のみ `audio_loop::start` を起動し select ループの新アーム `recv_audio` で `Msg::AudioFrame` を転送。**honest-disclosure: マイクにはフォールバックせず monitor/loopback デバイスのみ使用**(Linux は PulseAudio/PipeWire の monitor ソースで即動作、macOS/Windows は BlackHole/VB-Cable 等の仮想デバイスが要る — ネイティブ loopback は follow-up)。デバイス不在なら `audio:false` で劣化。viewer 側は受信・再生実装済みで変更不要 | 完了。次は Rust ビルド検証 + 実機での loopback E2E(接続→音声→切断)。**残: (a) ネイティブ WASAPI/CoreAudio loopback(仮想デバイス不要化)、(b) 複数 viewer 同時接続時の共有キャプチャ** — 現在は `handle_viewer` が接続ごとに spawn され上限が無いため各セッションが独自に capture を開く。システム音声はシングルトン資源なので2人目以降はデバイス使用中で失敗しうる(warn ログを出しそのセッションのみ無音 = クラッシュはしない)。正しくは1キャプチャを broadcast で全セッションへファンアウト |
| 2 | ~~HW エンコード (AV1/H264/H265)~~ → **実装済み(実エンコード検証済み)** | `ffmpeg_enc.rs` は `bail!` するスタブで、しかも **`mod` 宣言が無く rustc が一度もコンパイルしていなかった**。`ffmpeg` feature も依存クレートをゼロ個しか引かない空フラグだった | **完了**。`ffmpeg-next` は**使わない**。libavcodec は C ライブラリであり、crates.io は不要だと再判定した — 束縛の難所は `AVCodecContext`/`AVFrame`/`AVPacket` が直接アクセス構造体で ffmpeg メジャー間にレイアウト差がある点だけなので、**C シム(`csrc/miru_ffmpeg.c`)を実ヘッダでコンパイルして構造体を C 側に閉じ込め**、Rust には不透明ポインタとスカラーだけを渡す。結果として **`ffmpeg` feature は Rust 依存を1つも増やさない**(マスク「最良の部品は部品が無いこと」)。構成: シム → `build.rs`(`cc` / `pkg-config` クレートではなく `std::process::Command` で cc と pkg-config を直接呼ぶ)→ `platform/ffmpeg_ffi.rs`(本 crate の `unsafe` は全てここ = ワークスペース規約)→ `ffmpeg_enc.rs`。低遅延設定(B フレーム0、nvenc `p1`/`delay=0`、x264 `zerolatency`、qsv/vaapi `async_depth=1`、amf `ultralowlatency`)。`recv` は `_begin`(サイズ通知)/`_copy`(取り出し)の2段でパケット切り詰めを構造的に排除。**検証**: `verify-offline.sh` に実コンパイル+実行を追加し、mpeg4 で **10フレーム→実パケット・実バイト・キーフレーム1件**の完全な送受ループが通ることを確認(テスト6件)。HW エンコーダは**コーデック名が違うだけで同一コードパス**であり、この環境の libavcodec 60 には `h264_nvenc`/`av1_nvenc`/`h264_vaapi`/`hevc_vaapi` が全て存在することも確認済み | **配線も完了**: `Encoder::new` が `probe()` の順に HW エンコーダを試し、**最初に open できたもの**を採用。`available_codecs()` は「実際に open できた HW コーデックのみ」を広告する(`OnceLock` で1回だけ実測)— `h264_nvenc` は GPU 非搭載機の libavcodec にも含まれるため、**存在と開けることは別概念**として検査する(FFI テストで固定)。既存不変条件「広告した全コーデックは `Encoder::new` で構築可能」を維持し、`has_hw_encode()` も派生値として自動的に正しくなる。**残**: GPU 実機での nvenc/vaapi 実行確認と `cargo test --workspace` |
| 3 | ~~WebGL2 YUV 描画パス~~ → **WebCodecs パスとして実装(要 cargo test / 実機検証)** | ADR 0013 の通り WebGL2 に生 I420 を IPC 転送する設計は帯域 ~15x 退行のため凍結が正しく、真の解は WebCodecs `VideoDecoder`。実装済み: host が VP9/VP8 ビットストリームをそのまま `video-packet` イベントで転送(`session.rs`、`webcodecs_decode: AtomicBool` で分岐)、フロントは `lib/webcodecs-renderer.ts` の `VideoDecoder` でデコードして canvas 描画。`VideoDecoder.isConfigSupported` で対応判定し、デコードエラー時は `set_decode_mode(false)` で Rust JPEG パスへ自動フォールバック。録画中は JPEG が必要なため WebCodecs を自動的に無効化。TypeScript は `tsc --noEmit` でエラーゼロを実機検証済み。`yuv-renderer.ts` は参照用として凍結のまま(将来削除候補) | 完了。次は Rust ビルド検証 + 実機での VP9 セッション描画・フォールバック確認 |
| 4 | Wayland / PipeWire キャプチャ | `platform/linux.rs` の `PipeWireCapturer` が `bail!` するのみ。Ubuntu 22.04+ / Fedora の標準セッション(Wayland)で動かない | **交渉部分は実装済み・実ポータルで検証。フレーム取得は未実装** — 完了ではない。項目2と同じく `pipewire` クレートは**使わない**: `csrc/miru_portal.c`(GDBus + libpipewire の C シム)+ `build.rs` + `platform/portal_ffi.rs` で **Rust 依存ゼロ**(`Cargo.toml` から `pipewire = "0.8"` を削除)。CreateSession → SelectSources → Start → OpenPipeWireRemote の全段を実装し、**失敗したステップ名と D-Bus エラー文を型付きで返す**(`PortalError { stage, detail }`)— 「ポータルが拒否した」ではなく「SelectSources が拒否された」と言えることが運用上の差になる。**検証環境をコンテナ内に構築した**: sway(headless)+ pipewire + wireplumber + xdg-desktop-portal(-wlr)を起動し、実ポータル(ScreenCast v5)に対して **CreateSession と SelectSources の成功を確認**(バックエンドが `HEADLESS-1` を選択するところまでログで追跡)。**`Start` はこの構成では失敗する**: xdpw が `unable to receive a valid format from wlr_screencopy` を出す — DRM デバイスの無い headless sway が提示するピクセルフォーマットを xdpw が受け付けないためで、**クライアント側の欠陥ではない**(同じ環境で `grim` は screencopy 経由で実際にスクリーンショットを取得できる)。 | **残**: (a) PipeWire ノードからのフレーム取得(本体)、(b) `Start` を通せる環境での E2E。wlroots 系限定なら `zwlr_screencopy_v1` 直接利用がこの環境でも完全に検証可能な代替経路(grim が実証) |
| 5 | macOS 60fps キャプチャ | `crates/miru-capture/src/platform/macos.rs` — deprecated な CGDisplayCreateImage で ~20fps | ScreenCaptureKit (SCStream) へ移行。規模: 中 (1-2週) |
| 6 | CI/CD が稼働していない | ワークフローが `.github/workflows/` ではなく `.github/workflows-proposed/` に隔離されたまま。push する GitHub App に `workflows` 権限が無く自動化エージェントでは移動できない(実際に push が拒否されることを確認済み) | **リポジトリ管理者の手作業が必要**: `git mv .github/workflows-proposed/*.yml .github/workflows/`。規模: 極小 |
| 7 | ~~クライアント側 TOFU 確認ダイアログが完全に未接続~~ → **実装済み(要 cargo test 検証)** | `crates/miru-client/src-tauri/src/session.rs` のハンドシェイク直後に TOFU ゲートを追加: viewer 側 `AclStore`(`crates/miru-auth/src/lib.rs`)の `check()`/`trust()`/`touch()` を初めて呼び出すよう配線。`TrustDecision::Unknown`(初回接続)は `pending_pairing` oneshot チャネル経由でフロントエンドの確認を待ち(120秒タイムアウト)、`PairingDialog.tsx`(`requirePin={false}` モード)で指紋を能動的に確認させてから `TrustedPeer` を永続化する。`TrustDecision::PubkeyMismatch`(鍵変更 = MITM の可能性)は `NoAutoRetry` マーカーエラーで即座に拒否し、再接続ループが自動リトライしないようにも対応済み。新規 Tauri command `confirm_pairing`。TypeScript 側は `npx tsc -b --noEmit` でエラーゼロを実機検証済み。**Rust 側はサンドボックスの crates.io 遮断によりコンパイル未検証** — マージ前に必ず `cargo test --workspace` を実行すること | 完了。次のアクションは Rust ビルド検証のみ |
| 8 | ~~ホスト側 TOFU が無条件自動承認~~ → **オプトイン強化モードを追加(要 cargo test 検証)** | `crates/miru-host/src/session.rs` の `check_or_pair()`。`miru-host` は GUI の無い純粋な CLI/ヘッドレスバイナリのため viewer 側と同じ対話的確認ダイアログは実装できない。デフォルト挙動(初回接続は自動承認)を無断で変える判断は破壊的変更のリスクがあるため、**デフォルトは変更せず** `HostConfig::require_pairing_confirm`(`MIRU_REQUIRE_PAIRING_CONFIRM=1` で有効化、`main.rs` で一度だけ読み取り)を追加。`MIRU_ALLOW_STUB` の「デフォルト維持・明示的 opt-in」パターンを踏襲 | 完了。次のアクションは Rust ビルド検証のみ |
| 9 | ~~`ConnectArgs.pin` フィールドが実質デッドコード~~ → **誤解を招く入力欄を削除(tsc 検証済み)** | 検証の結果、PIN は `ConnectScreen` → `api.connect` → `ConnectArgs.pin` と流れるが `session.rs`/`state.rs` のどこからも読まれない完全なデッドコードと確定。初回接続のセキュリティは実際には TOFU 指紋確認ダイアログ(項目7)であり、何も強制しない PIN 入力は「保護されている」という誤った安心感を与えるため削除。UI 入力・`api.connect` の `pin` 引数・`ConnectArgs.pin` フィールドを一括除去。PIN ペアリング自体は `CLAUDE.md`(暗号/PIN、PBKDF2-SHA256)の将来機能として維持し、ホスト側の実強制と配線する時点で入力欄を再追加する旨をコード内コメントに明記 | 完了(将来 PIN 機能実装時に再配線) |
| 10 | ~~UI アクセシビリティ欠如~~ → **解消済み(12/12コンポーネント)** | 商用品質監査(2026-07)で確認: 12個の `.tsx` 全体で `aria-*`/`role=` 使用が実質ゼロだった。全コンポーネントに基礎対応済み: label/input 関連付け、`role="dialog"`/`"alert"`/`"status"`、`aria-live`、ナビゲーションの `aria-current`、識別可能な `aria-label`。特筆: `TimelineScrubber.tsx` のシークバーはキーボード操作が皆無だったため `role="slider"` + 矢印キーハンドラを新規実装、`ConstellationMap.tsx` の SVG可視化はキーボード到達不能な複製UIのため `aria-hidden` で隠し実アクセス手段(device-list の `<button>`)のみ露出。全て `tsc` 検証済み | 完了。今後は実スクリーンリーダー(NVDA/VoiceOver)での実機検証が望ましい |
| 11 | ~~存在しない `miru.app` ドメインへの依存~~ → **解消済み** | `MIRU_SIGNAL` のデフォルト値が未登録の `signal.miru.app` を指していた(`main.rs`/`scripts/install.sh` 双方)。`PRIVACY.md` は実装ゼロのクラッシュレポート機能を実在するかのように詳述していた(`--enable-crash-reports` フラグはコード上皆無、`grep` で確認済み)。全て `localhost` デフォルトへの修正、または「未実装/未稼働」の正直な注記に置換 | 完了 |
| 12 | ~~`PairPrompt.tsx` が完全なデッドコード~~ → **削除済み(tsc + Vitest 検証済み)** | import 0件を再確認のうえ `PairPrompt.tsx`(102行)と、ADR 0013/0020 で WebCodecs に置換され同じく参照ゼロだった `yuv-renderer.ts`(214行)を削除。計316行。当初「プロダクト判断待ち」としていたが、**git が履歴を保持する以上、参照ゼロのコードは削除が既定**であるべきと判断を改めた(ADR 0022)。必要になれば戻せる | 完了 |

## 🔴 追加(2026-08 First Principles 再監査 — 入力経路)

「①画面が見える ②操作できる」が中核という第一原理から入力経路を精査した結果、
**従来の監査が一件も捉えていなかった中核機能の破綻**を発見。13〜15 は本ブランチで修正済み。

| # | 項目 | 根拠 | 状態 |
|---|------|------|------|
| 13 | **キーコードが無変換で3つの別番号空間へキャストされていた(最重要)** | ビューアは `e.keyCode`(Windows VK 由来)を送信し、各バックエンドが変換せず OS API へ渡していた。evdev(`KEY_A`=30)/ CGKeyCode(`A`=0)/ VK(`A`=65)は**互いに無関係な番号空間**のため、`A` が Linux で `KEY_F7`、macOS で別キーになる。変換表はリポジトリ内に存在しなかった。`miru-mcp/src/server.rs:459` は "host translates per OS" と契約を書いていたが**ホストは変換していなかった** | **修正済**。`miru-input/src/keymap.rs` を新設し W3C `code`("KeyA")→ evdev / CGKeyCode / VK の変換表を実装(数値は Chromium `keycode_converter_data.inc` = USB HID Usage Tables + W3C UI Events 準拠)。プロトコルに `code: Option<String>`(`serde(default)`)を追加し双方向後方互換。`code` 不在時は旧来の生キャストへフォールバック。**2026-08 追補: JIS/テンキー/システムキーを追加し計120キーに拡充** — `IntlYen`(¥)・`IntlRo`(ろ)・`Convert`(変換)・`NonConvert`(無変換)・`KanaMode`(かな)は日本語環境で日常的に使うにも関わらず未収録だった。macOS に存在しないキー(変換/無変換/かな/PrintScreen/ScrollLock/Pause)は Chromium 同様 `0xffff` 番兵で表現し `None` を返す(65535 を実キーとして注入しない)。evdev・CGKeyCode は衝突ゼロを検証済み |
| 14 | **押しっぱなしキーが永久に解放されない** | 修飾キーの状態追跡もリリース処理も皆無。ビューアは `blur`/`visibilitychange` を監視せず、Alt+Tab 離脱で Alt-up が送られない。(**訂正**: 当初「`Ctrl+W` 早期 return により Ctrl-up が構造的に欠落」と記載したが、イベント列を再検証した結果**誤り**。Ctrl の keyup では `e.key` が `"Control"` でフィルタ対象外のため Ctrl-up は送出される。ただし再検証で**別の実バグ**を発見: Ctrl+W で W を押した後に **Ctrl を先に離す**と `e.ctrlKey` が false になり W の keyup だけがフィルタをすり抜け、**key_down 無しの key_up がホストへ送られる**。押下記録に無いキーは release しないようガードして修正済み。)Linux の uinput デバイスはプロセス寿命の `static` のため**stuck キーが再接続をまたいで残存** | **修正済**。ホスト: `InputHandler` が押下中キーを追跡し `release_all_keys()` をセッション終了時に実行 + `Drop` 実装で `?` 早期 return も含む全経路を網羅(単体テスト3件付き)。ビューア: 押下中キーを追跡し blur / visibilitychange / unmount で key_up を送出 |
| 15 | Linux スクロールの実装バグ2件 | `if dy > 0.0 { 1 } else { -1 }` により **`dy == 0.0` が「下スクロール」**になっていた(横スクロール専用イベントが縦に誤動作)。また `REL_HWHEEL` が uinput デバイスに未登録で**横スクロールが構造的に不可能**、`dx` は破棄されていた | **修正済**。ゼロ delta は no-op 化、`REL_HWHEEL` を登録し `dx` を注入 |
| 15b | `Ctrl+Q` / `Ctrl+W` / `Ctrl+H` / `Ctrl+M` をホストへ送れない | `SessionScreen.tsx` のキーボードハンドラは、ビューア自身のウィンドウが閉じられるのを防ぐためこの4つの組み合わせを早期 return で握り潰している。結果として**リモート機側でタブを閉じる/ウィンドウを隠す等の操作が一切できない**。リモートデスクトップとしては実用上の欠落 | **修正済(tsc + Vitest 検証済み)**。ツールバーに「特殊キー送信」を追加し、握り潰される4組をプルダウンで選んでホストへ送出できるようにした。MCP の `key_combo` と同じ方式で修飾キーを実キーイベントとして送る(Ctrl 押下 → キー押下/解放 → Ctrl 解放)。送信途中で失敗しても `finally` で必ず Ctrl を離すためホストに修飾キーが残らない。**Ctrl+Alt+Del は意図的に含めていない** — Windows は仕様上 `SendInput` から生成できず(項目16)、並べると「押しても何も起きないコントロール」= false affordance になるため |
| 15c | **ホイールスクロールが `deltaMode` を無視し行モードで壊れる** | `SessionScreen.tsx` の `onWheel` が `deltaY / 100` 固定で、`WheelEvent.deltaMode` を参照していなかった。MDN/W3C UI Events は「値がピクセルとは限らない。`deltaMode` を必ず確認せよ」と明記。行モード(DOM_DELTA_LINE)では1ノッチ `deltaY≈3`(行)なので `/100` = **0.03 = ほぼ無反応**。Firefox と **WebKitGTK(= Linux の Tauri webview そのもの)** は行モードを報告するため、**主要 Linux ターゲットでスクロールがほぼ効かなかった** | **修正済(tsc + Vitest 検証済み)**。純粋関数 `lib/wheel-normalize.ts` を新設し normalize-wheel 慣習(40px/行、800px/ページ)でピクセルへ換算後ノッチ単位(~100px/ノッチ)へ正規化。ピクセルモードは従来と完全一致(÷100)で回帰なし、行/ページモードのみ救済。単体テスト6件で回帰固定 |
| 15d | **ペアリング確認ダイアログ表示中の入力がバッファされ、確認直後にリモート機へ流し込まれる** | `AppState::connect`(`state.rs:132`)は**ハンドシェイク前に**セッションスロットを埋めるため `send_input` は成功し、容量64の mpsc チャネルへ**バッファされる**。このチャネルを drain するセッションループ(`session.rs:342`)が回り始めるのは**ハンドシェイク・TOFU 指紋確認・暗号鍵インストールの後**。したがって指紋ダイアログを読んでいる間のマウス移動と**キー入力**(キーリスナは `window` にあるため常時有効)が溜まり、ユーザーが「接続」を押した瞬間にまとめてリモート機へ注入される。**未確認のピアに対して行った入力が、確認後に実行される** | **修正済(tsc + Vitest 検証済み)**。`lib/input-sender.ts` を新設し、`status === "connected"` の時のみ送出するゲートを全入力経路(KeyTracker / MoveCoalescer / マウス・タッチの直接送出)に適用。準備状態は呼び出し毎に評価するため、effect 内で一度生成したセンダでも現在の接続状態を見る。あわせて fire-and-forget な送出の rejection を握り潰し、切断中の unhandled rejection を防止(単体テスト5件) |
| 15e | 映像ホットパスの base64 デコードが毎フレーム手書きループ + 実装が二重化 | `SessionScreen.tsx` と `webcodecs-renderer.ts` が同じ `b64ToBytes`(`atob` + 1バイトずつ `charCodeAt`)を**別々に**持ち、**フレーム毎にメインスレッドで**実行していた。1080p JPEG は ~270KB なので 1フレームあたり約27万反復、30〜60fps では描画と CPU を奪い合う | **修正済(tsc + Vitest 検証済み)**。`lib/base64.ts` に一本化し、**TC39 arraybuffer-base64(Stage 4 = 標準採択済み)の `Uint8Array.fromBase64`** をモジュール読み込み時に1度だけ機能検出してネイティブ実装を使用、無ければ従来ループにフォールバック。Tauri が埋め込む WebView は OS ごとに異なる(Linux=WebKitGTK / macOS=WKWebView / Windows=WebView2)ため機能検出が必須。両実装をエクスポートし**バイト単位で一致することを検証するテスト**を用意(ネイティブ非対応環境では `it.skipIf` でスキップ)。戻り値の型は `Uint8Array<ArrayBuffer>` に固定 — 既定の `ArrayBufferLike` は共有バッファの可能性があり `BlobPart` として不正で、JPEG 経路が `Blob` に包むため tsc が実際に検出した |
| 15f | ファイル送信の base64 エンコードで UI がフリーズ | `handleFileSend` が `binary += String.fromCharCode(bytes[i])` で**1バイトずつ**文字列連結していた。ファイルピッカーは 100MB まで許可するため**最大1億回の反復をメインスレッドで同期実行** — ウィンドウが数秒〜数十秒固まる | **修正済(tsc + Vitest 検証済み)**。`lib/base64.ts` に `bytesToBase64` を追加。同じ TC39 提案の**エンコード側 `Uint8Array.prototype.toBase64`** を機能検出し、非対応環境では 8192 バイト単位のチャンク変換にフォールバック(1億回 → 数千回)。チャンク境界(8191/8192/8193/16389 バイト)での破損を検出する回帰テスト付き。**ピーク메모리は未解決** — `send_file` コマンドが base64 を一括で受け取る設計のため、解消には Rust 側のチャンク IPC が必要(範囲外として明記) |
| 15g | **タッチ操作が対応しない `mouse_up` を送る / スクロール中も左ボタンが押されたまま** | `onTouchStart` は2本指時に `mouse_down` を送らないのに `onTouchEnd` が**無条件に**左 `mouse_up` を送信していた。`touchend` は指ごとに発火するため、2本指スクロールの終了時にホストへ**押下無しの解放が2回**届く。さらに1本目を押したまま2本目を置くと左ボタンが押されっぱなしになり、**左ドラッグ状態でスクロール**していた | **修正済(tsc + Vitest 検証済み)**。`lib/touch-gestures.ts` に左ボタン状態を追跡する純粋な状態機械を新設。押下が実際に未解放のときだけ解放を送り、2本目の指が加わった時点で解放する。unmount 時も解放。単体テスト10件(2本指スクロール終了で解放ゼロ、シーケンス全体で解放はちょうど1回、連続押下を作らない等) |
| 15h | ~~タッチで右クリックできない~~ → **実装済み(tsc + Vitest 検証済み)** | 1本指=左クリック、2本指=スクロールのみで、**右クリックの手段が存在しなかった**。比較実装(Guacamole / RustDesk モバイル / OS 標準のタッチ操作)はいずれも長押しか2本指タップを右クリックに割り当てている | **修正済**。当初「初回押下の遅延はドラッグ応答遅延というトレードオフがあり実機確認が必要」として見送っていたが、**この懸念は過大評価だった**と再判定。遅延するのは*静止した指*の押下のみで、(a) タップは `touchend` で press+release をまとめて送るため体感差ゼロ、(b) ドラッグは移動が slop(正規化 0.01 ≒ 1920px 幅で19px、Android の touch slop ~8dp と同等)を超えた瞬間に press を送り、かつ**押下座標を指が最初に着地した位置に刻む**のでドラッグ開始点はずれない。長押し判定は**タイマーではなく指を離した時**に行う — `now` を引数で受ける純粋な状態機械のままとなり、Node 環境の vitest で完全に検証できる(CLAUDE.md「シンプルさ優先」)。閾値 500ms は Android/iOS の慣習値。**副次的に実バグも解消**: 従来は2本指スクロール開始時に1本目が left press を送り2本目着地で release されるため**ホストへ毎回スクロール開始時の偽クリックが1回届いていた**。遅延設計では未確定の押下を破棄するだけなのでボタンイベントはゼロになる。プロトコル変更は不要(`MouseButton::Right` は既存)。テスト14件(タップ/長押し境界値 499・500ms/ドラッグの押下座標/2本指でボタンイベントゼロ/cancel が未確定を破棄) | **トレードオフ(承知の上)**: 「押しっぱなしにして動かさない」操作(リモート側ボタンの長押しリピート等)がタッチから行えなくなる。Guacamole / RustDesk モバイルと同じ選択で、右クリック手段がゼロだった現状より実用上の損失は小さいと判断 |
| 15i | **再接続バックオフにジッタが無く、シグナルサーバ再起動時に retry storm を起こす** | ホストの signal 再接続(`miru-host/src/session.rs`: `backoff_secs * 2`)もビューアのセッション再試行(`state.rs`: `1u64 << attempt`)も**完全に決定論的**で、リポジトリ全体で `jitter` の実装はゼロ。セルフホスト構成では全デバイスが同一の `MIRU_SIGNAL` を向くため、**サーバ再起動時に全ホストが同時に切断され、5s/10s/20s/40s と足並みを揃えて再接続**する。復帰しかけたサーバを再度倒しうる典型的な thundering herd。参考: Marc Brooker "Exponential Backoff And Jitter"(AWS Architecture Blog, 2015)、Amazon Builders' Library "Timeouts, retries, and backoff with jitter" | **修正済(要 cargo test)**。`miru-common/src/backoff.rs` を新設し **equal jitter**(`sleep = temp/2 + rand(0, temp/2)`)を実装、ホスト・ビューア両方の再接続に適用。full jitter ではなく equal jitter を選んだ理由: full は 0 に近い待機を連続で引きうるため、**明らかに落ちているサーバへの再接続ループでは hammering に退化する**。equal は「計算値の半分」という下限を保証しつつ後半で分散する。乱数draw を引数に取る純粋関数に切り出し、単体テスト9件(下限/上限/指数増加/cap 飽和/分散/NaN・範囲外クランプ/`u32::MAX` でも panic しない)で固定 |
| 16 | Ctrl+Alt+Del(SAS)を送る手段が無い | `InputKind` に該当 variant が無く、Windows の `SendInput` では原理的に生成不可(設計上の制約)。ログイン画面・UAC プロンプト・ロック画面へ到達できない | **未着手**。`SendSAS` / セキュアデスクトップ対応が必要な独立課題。規模: 中 |
| 17 | **マルチモニタの座標指定が3OS全てで壊れている**(当初の記載より重症) | `MouseMove.display` は3バックエンドとも `..` で読み捨て。さらに調査の結果、**単なる「macOS でセカンダリへ移動できない」ではなく、非プライマリ表示中はクリック位置が全OSでずれる**と判明: (a) **macOS** — `screen_point()` が `CGDisplay::main()` の bounds 固定 (b) **Windows** — `MOUSEEVENTF_ABSOLUTE` を `MOUSEEVENTF_VIRTUALDESK` **無し**で使用。Microsoft 公式ドキュメント(MOUSEINPUT)は「マルチモニタ環境では座標はプライマリモニタにマップされる。`MOUSEEVENTF_VIRTUALDESK` を指定した場合のみ仮想デスクトップ全体にマップされる」と明記 (c) **Linux** — ABS 0〜65535 をコンポジタが仮想デスクトップ全体へマップするため、選択中ディスプレイ内の正規化座標とはそもそも意味が違う。加えて **`DisplayInfo` に x/y オフセットが無い**(`width`/`height`/`primary` のみ)ため、**プロトコル上マルチモニタ配置を表現できず構造的に修正不能** | **未着手 — 部分修正は危険**。`MOUSEEVENTF_VIRTUALDESK` を単独で足すと、プライマリ1枚を見ている通常ケースで (0.5,0.5) が「プライマリ中央」から「仮想デスクトップ中央」へずれる**退行**になる。座標変換とセットでしか直せない。必要な一括変更: ①`DisplayInfo` に `x`/`y` オフセット追加(各OSの列挙も対応) ②ホストが「選択ディスプレイ内の正規化座標」→「仮想デスクトップ絶対座標」へ変換 ③Windows は `MOUSEEVENTF_VIRTUALDESK` 併用 ④macOS は対象ディスプレイの origin+bounds を使用(CGEvent 座標はグローバル空間) ⑤`MouseDown`/`MouseUp` には `display` フィールドが無い点も要解決。**ビルド・実機検証必須**のため本サンドボックスでは着手しない。規模: 中 |
| 18 | Linux の `Text` 注入がクリップボードを破壊する | `platform/linux.rs` はクリップボードへ書いて Ctrl+V を合成する実装。(a) ユーザーのクリップボード内容を毎回破壊 (b) `xclip` 依存で**X11 限定** (c) Ctrl+V が貼り付けでない端末等では動作しない — `Text` の主用途である MCP エージェントの `type_text` は端末打鍵が多いため主用途での破綻 (d) クリップボード書き込み失敗時に `Ok(())` を返し**何も入力していないのに成功と報告**していた | **部分対応 — ADR 0021 に判断を記録**。(d) と冒頭コメントの誤記は修正済み。**方式変更(キー合成)は意図的に見送り**: uinput は原理的にレイアウトを知り得ず、参照実装の ydotool も設定レイアウトを無視して US QWERTY で打鍵する既知の未解決制限を抱える(ydotool #43: Dvorak で `This is a test`→`Ydco co a y.oyv`)。単純置換は「非US配列の X11 ユーザーで現在正しく動くものを誤入力にする」退行。貼り付け前後でのクリップボード退避/復元も、貼り付けが非同期のため復元が早すぎると**古い内容がリモートアプリへ挿入される**(消失より悪い)ため、テスト不能な環境での盲目実装を回避。**正しい解**: Wayland は `zwp_virtual_keyboard_v1` で自前キーマップを上げる(`wtype` 方式、制限なし)、X11 は XKB 問い合わせで keycode+修飾を解決(`xdotool` 方式)。項目4(Wayland 対応)と同時実装が自然 |
| 19 | ~~MCP エージェント経路は依然として旧キャストのまま~~ | `miru-mcp/src/server.rs` の `parse_key` は VK コードを生成し `code: None` で送るため、項目13のフォールバック(生キャスト)経路に乗っていた | **修正済**。`parse_key_code()` を追加し W3C `code` を併送。「送出する全 code が `miru_input::keymap` で解決できること」を検証するテストを同梱(ホスト側テーブルとの不整合を機械検出) |
| 20 | **`key_combo` の修飾キーが Linux/Windows で完全に無視されていた** | `tool_key_combo` は "Ctrl+Shift+T" を `modifiers` ビットマスクに畳んで送っていたが、このフィールドを読むのは **macOS バックエンドのみ**(`macos.rs:68,75`)。Linux(`linux.rs`)と Windows(`windows.rs`)は `..` で読み捨てるため、**`Ctrl+C` が単なる `c` として注入されていた**。エージェントのショートカット操作が2/3のOSで機能していなかった | **修正済**。ビットマスクだけでは和音を表現できないため、修飾キーを**実キーイベント**として送出する方式に変更: 修飾キー押下 → 主キー押下/解放 → 修飾キーを**逆順で**解放。`modifiers` も macOS の CGEventFlags 用に併送し続ける。和音の途中で送信が失敗した場合はホスト側の `release_all_keys()`(項目14)が回収する |


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
| 5 | テスト文化 | 統合テスト・ベンチ (`crates/miru-bench/`)・ファズ (`fuzz/`)・リグレッションテストが機能追加に伴い整備されている。フロントエンドは従来テスト皆無だったが Vitest を導入し純粋ロジック(WebCodecs のコーデック判定・キーフレームゲーティング・base64 デコード)にテスト追加 (`npm test`、10件パス) |

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

---

## 削除候補の実測 (Musk ステップ②) — 2026-08

`scripts/find-dead-code.py` により、**ワークスペース内のどこからも参照されない
`pub` 項目が 20 件**あることを確認した。うち最大のものを記録する。

### `miru-transport/src/quic.rs` (319行) — 全体が未参照

| 項目 | 実測 |
|------|------|
| `QuicTransport` / `QuicSendStream` / `QuicRecvStream` の参照 | **0件**(ファイル外) |
| `quic::` の参照 | **0件**(ファイル外) |
| `quinn` を使う他のファイル | **0件** |
| `rcgen` を使う他のファイル | **0件** |

つまり `quic.rs` を削除すれば **319行 + `quinn` + `rcgen` 依存2件**が
既定ビルドから消える。`publish = false` なので外部利用者もいない。

**それでも本 PR では削除しない。** 理由:

1. `docs/architecture.md` / `THREAT_MODEL.md` / `competitive-analysis.md` /
   `RESEARCH_NOTES.md §3 (MoQ)` / CLAUDE.md のポート表が、QUIC を
   **製品の設計方針として明示**している。削除は製品判断であって清掃ではない。
2. `docs/DEVELOPMENT_LOG.md` に「QUIC stream 定数を `#[allow(dead_code)]` +
   `pub const` に変更」という**意図的に残した記録**がある。記録された判断を
   エージェントが独断で覆すべきではない。

判断が必要な問いは1つ:
**QUIC 優先パスは着手予定か、それとも WS リレー + hole-punch で確定か。**
前者なら残す。後者なら 319行と依存2件は即座に削除できる(git が保持する)。

### 本 PR で削除したもの

- `miru-transport/src/tls.rs: assert_secure_mode()` — 名前は `assert_` だが
  **ログを出すだけで何も強制せず、しかもどこからも呼ばれていない**。
  ガードに見えて実効ゼロという最悪の形。削除しても実行時挙動は変わらない
  (呼ばれていないため)。警告自体が必要なら `main()` から
  `warn_if_insecure_tls_requested()` として呼ぶのが正しい形。

### 2026-08 追補: 20件の最終処分

| 群 | 件数 | 処分 |
|----|------|------|
| コア crate の自明なアクセサ(`try_next_frame` / `is_full_frame` / `on_audio_frame` / `skip_rate` / `rtt_min_ms` / `update_rtt`) | 6 | **削除済み**。`#[allow(dead_code)]` は「消すな」ではなく警告を黙らせた記録に過ぎず、git が履歴を保持する |
| `quic.rs` | 9 | 保留 — 上記の製品判断待ち(本節冒頭) |
| MCP/agent 凍結スコープ(`INVALID_REQUEST`/`TOKEN_INVALID`/`TOOL_DENIED`/`read_all`) | 4 | 保留 — roadmap §0 の凍結宣言に従い、凍結中コードは個別に削らない |
| `safe_fs::open_for_read` | 1 | **残す** — O_NOFOLLOW 付きの安全 open。doc が file 送信ホスト側での使用予定を明記したセキュリティ API |

残る判断は quic.rs の1問のみ。scan の健全性は fuzz ターゲット参照
(`parse_stun_xor_mapped_fuzz` が正しく除外される)で確認済み。
