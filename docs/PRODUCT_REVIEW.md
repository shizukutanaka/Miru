# Miru v0.1.0 プロダクトレビュー — 長所・短所・改善記録

2026-06 実施。3系統(セキュリティ系クレート / アプリ・メディア系クレート /
実ビルド・テスト・CI 検証)でコードを実際に読み、rustc 1.88/1.94 で
ビルド・テストを実行して検証した結果。

## 長所(実コードで確認済み)

### 暗号・プロトコル設計
- **X25519 ephemeral + ChaCha20-Poly1305**: PFS あり、方向別サブ鍵
  (HKDF `miru-h2v-v1` / `miru-v2h-v1`)で両端の独立カウンター —
  ノンス衝突が構造的に不可能
- **`SessionCipher` の Clone 禁止**: ノンス再利用をコンパイル時に封じる設計。
  複製が必要な場合は `derive_subkey` のみが正規経路
- **128-bit sliding-window replay 保護**: 受信側 (highest, bitmap) 追跡、
  順序逆転は許容しつつ再生は拒否
- **Ed25519 全署名 `verify_strict` (ZIP-215)**: small-order 鍵・非正準 R を
  拒否。CI の crypto-gate が非 strict `verify(` を grep で検出して fail させる
- **ハンドシェイク**: challenge に ephemeral + identity 両鍵を束縛、
  プロトコルバージョン検査、相互署名検証

### AI エージェント基盤
- **Capability トークン**: 11 capability / 3層セキュリティレベル
  (Normal / Conditional / Dangerous)/ TTL ≤15分 hard cap / JTI revocation /
  発行者 pubkey ピン留め。JWT を使わず algorithm-confusion を回避
- **hash-chain 監査ログ**: SHA-256 連鎖 + 即時 fsync + `append()` 経由の
  強制 redaction(バイパス経路なし)
- **redaction matrix**: capability ごとに len+SHA-256+tag へ置換、URL は
  origin のみ、ScreenRead はフレームハッシュで visual-prompt-injection の
  事後トレーサビリティを確保
- **MCP サーバ**: stdio、親プロセス allowlist(Claude Desktop / Cursor /
  Codex)、per-capability rate limit + daily cap

### その他
- **TOFU fingerprint log**(HMAC chain)、**safe_fs**(symlink 拒否 /
  canonicalize / allowlist prefix — RustDesk CVE-2026-2490 対策)
- **バックプレッシャ設計**: bounded channel(8フレーム)+ AIMD QoS +
  BBR 風コントローラ
- **ドキュメント**: THREAT_MODEL(STRIDE)/ CRYPTO(invariant +
  transcript)/ INCIDENT_RESPONSE / ADR 12本 / UNWRAP_TRIAGE
- **CI の発想**: crypto invariant gate、unwrap budget、cargo-deny、
  SBOM + Sigstore keyless 署名(release)

## 短所と対応状況

### ✅ 今回修正済み (2026-06)

| # | 問題 | 修正 |
|---|------|------|
| 1 | toolchain pin 1.80 が Cargo.lock(edition2024 / rustc 1.88 要求)と矛盾し、pin どおりでは一切ビルド不能 | pin / rust-version を 1.88 へ。CI に MSRV ジョブ(pinned toolchain + `--locked`)追加 |
| 2 | `miru-client` がコンパイル不能(`SignalClient::connect` の引数ドリフト) | identity pubkey を渡す 3 引数呼び出しに修正 |
| 3 | `miru-mcp` の e2e テストがコンパイル不能(dev-dependencies 欠落) | `rand` を dev-deps に追加 |
| 4 | `miru-codec` の `vpx` feature がコンパイル不能(`_fps`/`_bitrate_kbps` の参照ミス) | パラメータ名修正 |
| 5 | Rekor 透明性ログが実行時必ず失敗(reqwest に TLS backend なし) | `rustls-tls` 有効化 + `tracing` 依存追加(doctest も修復) |
| 6 | **KeyType の 64 byte 未満の入力テキストが監査ログに平文記録**(短いパスワード漏洩) | 長さ問わず常時 redact(len + SHA-256 + tag)に変更 + テスト |
| 7 | ClipboardRead 応答の redaction が「呼び出し側の規律」頼み | `redact_action` 側でも `data`/`text` を redact(defense-in-depth)+ テスト |
| 8 | トークンの `iat > exp` が検証されない / clock skew 300s がマジックナンバー | `iat <= exp` 検証追加、`MAX_CLOCK_SKEW_SECS` 定数化 + テスト |
| 9 | revocation list / auto-approve キャッシュが無制限成長(メモリ枯渇 DoS) | 読込 1M 件上限、キャッシュ 1000 件 FIFO + scope key 4KiB 上限 + テスト |
| 10 | signal サーバの relay slot リーク(ピア未到達/未クレームで DashMap に永久残留) | タイムアウト時の削除 + 未クレームスロットの 60s reaper |
| 11 | 監査ログ作成時に一瞬 world-readable | `OpenOptions::mode(0o600)` で作成時に設定 |
| 12 | qos_bbr の guard 距離のある `unwrap()` | if-let / match でパニック経路除去 |
| 13 | CI lint/coverage が GTK 依存欠落で必ず fail / smoke test が `\|\| true` で常時成功 / `--locked` なし | 依存追加、smoke の grep を判定に昇格、全 cargo 呼び出しに `--locked` |
| 14 | release.yml: cosign-installer の SHA が 39 文字で無効 / Linux apt 依存なし / aarch64 リンカなし / README が案内する `miru-mcp` が配布されない | すべて修正、`miru-mcp` を配布物に追加 |
| 15 | deny.toml: `allow-build-scripts = []` は「全 build script 禁止」の意味で bans check が fail / path 依存が wildcard ban に抵触 | キー削除 + `allow-wildcard-paths = true` |
| 16 | rustfmt 523 diff / clippy 116 警告で lint job が通らない(「0 warnings」バッジと矛盾) | `cargo fmt` + `clippy --fix` 全適用 |
| 17 | README の数値が古い(89/89 テスト、ADR 0001-0009、rustc 1.80) | 実態(173 テスト、ADR 12本、rustc 1.88)に更新 |
| 18 | ヘッドレス host の確認コールバックが無言で `false`(なぜ拒否されたか分からない) | 拒否理由を warn ログに出力。Dangerous 操作の常時拒否は safe-by-default として維持 |

### ⏳ 未対応(設計済み・別スプリント向け)

優先度順:

1. **Wayland / PipeWire キャプチャ**(`miru-capture/src/platform/pipewire.rs` がスタブ)
   — Ubuntu 22.04+ / Fedora のデフォルトセッションで動かない。
   xdg-desktop-portal (zbus) + pipewire-rs で実装。規模: 中(2-3週)
2. **エージェント確認 UI** — Dangerous capability(ShellExec / FileWrite /
   ClipboardRead)はヘッドレスでは常時拒否。ネイティブ確認ダイアログ
   (v0.3 予定)が入るまで AI エージェントは Normal/Conditional のみ
3. **macOS ScreenCaptureKit 移行**(現状 deprecated な CGDisplayCreateImage、
   ~20fps)— SCStream で 60fps へ。規模: 中(1-2週)
4. **ハードウェアエンコード**(`miru-codec/src/hw.rs` は probe のみで
   encode は常に bail、`ffmpeg_enc.rs` は未配線)— `available_codecs()` が
   H265 等を広告しないようにするか、ffmpeg-next を配線する。
   暫定では JPEG / VP9(feature `vpx`)のみが実コーデック
5. **XRandR マルチモニタ**(`linux.rs` は常に単一ディスプレイを返す)
   規模: 小(数時間)
6. **未配線モジュールの統合**: `miru-host` の backpressure / metrics /
   recording / safe_fs / qos_bbr は実装済みだが session ループに未配線
   (`#[allow(dead_code)]`)。配線して E2E で検証する
7. **Merkle tree の RFC 6962 非準拠**(odd leaf を self-hash)— 内部一貫性は
   あるが他実装と相互運用不可。参照実装との比較テストを足すか準拠に戻す
8. **音声コーデックのネゴシエーション**(host が Opus を hard-code)
9. **CHANGELOG の整理** — 現在は開発セッションログ(Sprint 22-26 形式)。
   公開リリースノート形式(Keep a Changelog)への移行を推奨
10. **StubBridge のフェイルラウド化** — `$DISPLAY` 無しで黙って 1x1 PNG を
    返すより、明示フラグ(`MIRU_ALLOW_STUB=1`)が無ければエラーにする
