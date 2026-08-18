# Miru 実装指示書 — Opus / Sonnet 向け

作成: 2026-07 / 対象ブランチ: `claude/sweet-franklin-l5a1yw`(PR #1)

これは **後続の LLM エージェント(Claude Opus / Sonnet)が追加調査なしで着手できる**
ことを目的とした実行指示書です。長所短所は本ブランチの ~45 コミット反映**後**の
現状評価です。監査の全履歴は `docs/FEATURE_AUDIT.md`、外部調査は
`docs/RESEARCH_NOTES.md`、モデル使い分けの一般論は `docs/MODEL_GUIDE.md` を参照。

---

## 0. 全タスク共通の前提(最初に読む)

1. **最優先タスクは `cargo test --workspace` の初回実行**。本ブランチの Rust 変更の
   多くは crates.io 遮断環境でコミットされている(各コミットメッセージに明記)。
   ビルドできる環境に入ったら何より先にこれを実行し、エラーをタスク E で潰すこと。
   **実際にこの経路で 1 件のコンパイルエラーが混入した**(`SessionEvent` にフィールドを
   追加した際に `state.rs` の構築サイトが追随せず)ため、この警告は理論ではない。
2. **crates.io が無くても回る検証がある**: `scripts/verify-offline.sh`。
   全 `.rs` の構文チェック(rustfmt はパースのみで依存解決不要)+ std だけに依存する
   モジュール(`keymap` / `backoff`)のテストを素の rustc で**実際に実行** + フロントの
   tsc/vitest。**`cargo test` の代わりにはならない**(クレート境界の型エラーは検出不可)が、
   「ビルドできないから何も確認していない」を防ぐ。
   ピンされたツールチェーンが取得できない環境では `RUSTUP_TOOLCHAIN=stable` を付ける
   (`ls ~/.rustup/toolchains` で確認)。**rustc 自体はインストール済みでも
   `rust-toolchain.toml` のピンが取得不可だと全体が使えなくなる**点に注意。
3. フロントエンドは検証済み: `cd crates/miru-client/ui && npm run typecheck && npm test`
   (tsc エラーゼロ + Vitest 69件パス)。
4. コミット規約: Conventional Commits (feat/fix/perf/refactor/test/docs/chore)。
   機能追加はテスト同 PR 必須。`unwrap()` 禁止(CI に予算ゲートあり、現在25個)。
   `unsafe` は `platform/` 配下のみ。
5. CI はまだ `.github/workflows-proposed/` に隔離されている(リポジトリ管理者が
   `.github/workflows/` へ移動するまで動かない — 自動化エージェントには
   `workflows` 権限がなく移動不可)。

## 1. 長所 — 維持すべきもの(壊すな)

| 長所 | 根拠 | 注意 |
|------|------|------|
| 暗号層 | X25519 ephemeral + ChaCha20-Poly1305 方向別鍵 + リプレイ保護 (`miru-common/src/crypto.rs`)、Ed25519 `verify_strict` | CI の crypto-gate が非 strict verify / SessionCipher Clone を機械的に拒否する |
| viewer TOFU ゲート | 本ブランチで実装 (`miru-client/src-tauri/src/session.rs` のハンドシェイク直後)。鍵不一致は `NoAutoRetry` で即拒否、初回は指紋確認ダイアログ | クリックスルー override を追加しないこと(設計意図) |
| WebCodecs デコード経路 | ADR 0013→0020。VP9 パケット直送 + `VideoDecoder`、JPEG 経路はフォールバックとして併存 | JPEG 経路を削除しないこと(WebKitGTK 非対応環境・録画が依存) |
| QoS / バックプレッシャ | BBR 風制御 (`miru-host/src/qos_bbr.rs`) + atomic backpressure | |
| honest-disclosure 方針 | 未実装機能を実在するように書かない。`miru.app` 依存は全除去済み | 未登録ドメイン・未稼働メールを復活させないこと |
| テスト文化 | Rust 統合テスト/ベンチ/ファズ + フロント Vitest (`npm test`) | 新機能には必ず対応テストを同時追加 |

## 2. 短所 — 現時点で残る弱点

1. **音声が一切流れない**(エンコーダ/デコーダ/再生は完成済みなのにキャプチャ層が無い)
2. **HW エンコード無し**(実コーデックは JPEG + ソフトウェア VP9 のみ)
3. **Wayland で動かない**(Ubuntu 22.04+/Fedora 標準セッション)
4. **macOS が ~20fps**(deprecated CGDisplayCreateImage。Sequoia 15.1+ では
   旧 API 使用アプリに警告表示 = 互換性ブロッカー、RESEARCH_NOTES §7)
5. **CI 未稼働**(§0-4)・**本ブランチの Rust がコンパイル未検証**
6. AI エージェント統治 6 crate はコア完成まで**凍結**(roadmap §0 の意思決定)

## 3. 改善案 → タスク割当表

推奨モデルの根拠: Opus = 複数 crate 横断・OS API 判断・設計判断を含むタスク。
Sonnet = 統合点が明確で機械的に進められるタスク(`docs/MODEL_GUIDE.md` の実績準拠)。

### タスク A: ホスト側システム音声キャプチャ 【Opus 推奨 / 効果: 最大】

コア体験の最大欠落。**以下の事実は 2026-07 にコード探索で確認済み**(着手時に
行番号のズレのみ再確認せよ):

- `miru-audio` は `lib.rs` + `playback.rs` の2ファイルのみ。**capture モジュールは
  存在しない**。追加すべきは「OS 音声 → interleaved f32 48kHz」の層だけ:
  - `AudioEncoder::new(layout: Layout, bitrate_kbps: u32) -> Result<Self>`
  - `AudioEncoder::encode(&mut self, samples: &[f32]) -> Result<Vec<AudioFrame>>`
    — 960 サンプル(20ms@48kHz)単位で内部バッファリングし seq 採番まで済ませて
    返す。呼び出し側はチャンク境界を気にしなくてよい
  - `Layout::from_channels(n: u8)` は 1/2/6/8 を受理
- `cpal = "0.15"` は **workspace 依存に既にある**(miru-audio の playback が使用)。
  `miru-host/Cargo.toml` には cpal も miru-audio も**無い** → 両方の追加が必要
- 統合点(`crates/miru-host/src/session.rs`):
  - `Features.audio: false` は 320-334 行付近(虚偽広告回避の修正コメント付き)。
    キャプチャ初期化成功時のみ true にする
  - メイン送信ループは 514-675 行付近の `tokio::select!`。video は
    `cap_rx.recv_async()`(flume)→ `relay.send_msg(&msg).await` の形。
    audio も同型に: キャプチャスレッド → `flume::bounded::<Msg>(8)`
    (`capture_loop.rs:40` と同じ容量・drop 方針)→ select に受信アームを追加
  - `HandshakeResult.selected_audio_codec` は host 側で**現在未読**
    (`handshake.rs:216` で設定済み)。Opus 選択時のみキャプチャを開始せよ
- OS 別実装方針(`docs/RESEARCH_NOTES.md` §5 に詳細):
  - macOS 14.6+: cpal の CoreAudio loopback 対応を利用
  - Linux: PulseAudio の `<sink>.monitor` ソースを入力デバイスとして列挙
  - Windows: WASAPI loopback (`AUDCLNT_STREAMFLAGS_LOOPBACK`) は
    **EVENTCALLBACK と併用不可**の既知の罠。cpal 未対応ならポーリング型で
    `wasapi` crate 補完
- 受け入れ条件: (1) loopback E2E テスト(`miru-host/tests/` の既存パターン踏襲)、
  (2) キャプチャ不可環境では `audio: false` のまま**劣化動作**(起動失敗にしない)、
  (3) viewer 側は既に受信・再生可能(`miru-client` の audio スレッド実装済み)なので
  viewer 変更は不要のはず — 変更が必要になったら設計を疑え

### タスク B: FFmpeg HW エンコード実配線 【Opus 推奨】

`crates/miru-codec/src/ffmpeg_enc.rs` が `bail!("FFmpegEncoder not yet wired")` の
スタブ。ソース内 TODO コメントに実装手順の骨子あり。完了後は
`miru-codec/src/lib.rs` の `available_codecs()` のコメント(HW コーデックを
広告しない理由)に従い、実動作確認後にのみ H264/H265/AV1 を広告に追加。

### タスク C: PipeWire/Wayland キャプチャ 【Opus 推奨】

`crates/miru-capture/src/platform/pipewire.rs` がスタブ。xdg-desktop-portal
(zbus) + pipewire-rs。RustDesk の実装教訓(PR #15063/#14543、discussion #15417、
COSMIC 対応)を `docs/RESEARCH_NOTES.md` §2 にまとめてある。

### タスク D: macOS ScreenCaptureKit 移行 【Sonnet 可】

`crates/miru-capture/src/platform/macos.rs` の CGDisplayCreateImage を
SCStream へ。単一ファイル・単一 OS で完結し既存の `ScreenCapturer` トレイトに
合わせるだけなので Sonnet で可。ただし `unsafe` は `platform/` 配下のみ規則を厳守。

### タスク E: cargo test 初回実行の後始末 【Sonnet 可 / 機械的】

`cargo test --workspace` → コンパイルエラーが出た場合、各コミットメッセージの
「未検証」宣言に該当する箇所を修正。修正は最小差分で、`git log --oneline` の
該当コミットを引用してコミットせよ。特に注意深く見るべき箇所:
- `miru-client/src-tauri/src/session.rs` の WebCodecs 分岐(`VideoPacketEvent`)と
  TOFU ゲートの `tokio::select!`
- `miru-sandbox/src/linux.rs` の landlock フォールバック(`net_handled` フラグ)
- `miru-codec/src/color.rs` の新モジュール参照
- `vpx.rs` の `VP9E_SET_TUNE_CONTENT` 定数名(vpx-sys の bindgen 生成名と一致するか)

### タスク G: マルチモニタ座標の一括修正 【Opus 推奨 / 要ビルド環境】

FEATURE_AUDIT 項目17。**非プライマリ表示中はクリック位置が3OS全てでずれる**。
調査済みの事実:

- `DisplayInfo`(`miru-common/src/message.rs`)は `width`/`height`/`primary` のみで
  **x/y オフセットを持たない** → プロトコル上マルチモニタ配置を表現できない
- **Windows**: `MOUSEEVENTF_ABSOLUTE` を `MOUSEEVENTF_VIRTUALDESK` 無しで使用。
  MS 公式 MOUSEINPUT ドキュメント曰く「マルチモニタでは座標はプライマリに
  マップされる。`MOUSEEVENTF_VIRTUALDESK` 指定時のみ仮想デスクトップ全体」
- **macOS**: `screen_point()` が `CGDisplay::main()` 固定。CGEvent のマウス座標は
  グローバル表示空間なので、対象ディスプレイの origin を足せば正しくなる
- **Linux**: ABS 0〜65535 はコンポジタが仮想デスクトップ全体へマップする
- `MouseMove` は `display` フィールドを持つが3バックエンドとも `..` で破棄。
  **`MouseDown`/`MouseUp` にはそもそも `display` が無い**

**⚠️ 部分修正は退行を招く**: `MOUSEEVENTF_VIRTUALDESK` だけ足すと、プライマリ
1枚を見ている通常ケースで (0.5,0.5) が仮想デスクトップ中央へずれる。
①〜⑤(項目17参照)を一括で行い、必ず実機のマルチモニタ環境で検証すること。

### タスク F: `PairPrompt.tsx` の削除可否 【人間の判断待ち — 実装するな】

`crates/miru-client/ui/src/components/PairPrompt.tsx` は import ゼロの完全な
デッドコード(ホスト GUI モード構想の名残)。削除かホスト GUI 計画かは
プロダクト判断であり、エージェントが勝手に決めないこと。

## 4. やってはいけないこと(全モデル共通)

- `miru.app` ドメイン・未稼働メールアドレスへの参照を復活させる
- TOFU の鍵不一致にクリックスルー(「それでも接続」)を追加する
- `SessionCipher` に `Clone` を derive する(AEAD nonce 再利用 = CI が落とす)
- Ed25519 の非 strict `verify()` を使う(CI crypto-gate が落とす)
- JPEG-over-IPC 経路の削除(ADR 0020 — WebCodecs 非対応環境と録画が依存)
- 未実装機能をドキュメントに「実装済み」として書く
- `unwrap()` の新規追加(CI 予算 25 個、超過は落ちる)

## 5. 検証チェックリスト(タスク完了の定義)

```bash
cargo test --workspace                                  # 全 Rust テスト
cargo clippy --workspace --all-targets -- -D warnings   # lint ゼロ
cd crates/miru-client/ui && npm run typecheck && npm test  # フロント
```

加えて機能タスク(A-D)は実機での動作確認(接続 → 映像/音声 → 切断)と、
対応する E2E/統合テストの追加を完了条件とする。
