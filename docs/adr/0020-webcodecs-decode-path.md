# ADR 0020: WebCodecs VideoDecoder 経路を実装し、JPEG 経路をフォールバックに残す

**状態**: 承認 (2026-07-17)

## コンテキスト

ADR 0013 は「二重コーデック(Rust で VP9 デコード → JPEG 再エンコード →
WebView で JPEG デコード)を真に消す正攻法は WebGL ではなく WebCodecs
`VideoDecoder` である」と結論し、対応状況が揃った時点での移行を将来課題として
明記していた。本 ADR はその移行を**実装する**にあたっての設計判断を記録する。

ADR 0013 が残していた未決事項は3点:
1. WebCodecs 非対応環境(特に Linux の WebKitGTK は VP9 対応が限定的)を
   どう扱うか。
2. ランタイムでデコーダが失敗したときの挙動。
3. 録画(JPEG フレームを保存する)との両立。

## 判断

**WebCodecs 経路を追加するが、JPEG-over-IPC 経路は削除せずフォールバックとして
残す。** どちらを使うかはランタイムで動的に切り替える。

- ホストは VP9/VP8 ビットストリームをそのまま `video-packet` イベントで転送し、
  WebView の `VideoDecoder` がデコードして canvas に直接描画する
  (`crates/miru-client/ui/src/lib/webcodecs-renderer.ts`)。
- 切り替えは viewer 側の `set_decode_mode` Tauri command → `AppState` の
  `webcodecs_decode: AtomicBool` で行い、`session::run` がフレームごとに読む。
- `yuv-renderer.ts` は **今回も削除しない**。WebGL2 経路は依然として誤った
  前提(ADR 0013 参照)に基づくため復活させないが、参照実装として凍結を継続。

## 理由

### フォールバックは削除ではなく併存させる

WebCodecs の VP9 対応は WebView 実装依存で、特に Linux WebKitGTK では
未対応または不安定な環境が現存する。JPEG 経路を消すと、そうした環境で
映像が一切出なくなる。両経路を持ち、対応可否をランタイムで判定して
選ぶことで、「対応環境では低帯域・低遅延、非対応環境でも確実に動く」を
両立する。

### 対応判定は静的機能フラグではなくランタイム能力検出で

`VideoDecoder.isConfigSupported({codec})` で VP9 対応を非同期に確認してから
のみ有効化する(Pike の「feature flag より実際の能力で分岐」)。ビルド時に
決め打ちすると、同一バイナリが多様な WebView 上で動く Tauri の実態と合わない。

### デコードエラーは即フォールバックし、セッション中は再試行しない

`isConfigSupported` が true でも、実ストリームで configure/decode が失敗する
ことはある(プロファイル不一致、ドライバ差異)。`VideoDecoder` の error
コールバックが発火したら `set_decode_mode(false)` で JPEG 経路に落とし、
そのセッションの残りは JPEG で通す。ping-pong を避けるため、失敗した経路を
セッション中に再有効化しない(`wcFailedRef`)。

### 録画中は JPEG 経路を強制する

録画は JPEG フレームを保存する設計(`state.rs` の RecordingState)。WebCodecs
経路では Rust 側で JPEG が生成されないため、録画中は WebCodecs を自動的に
無効化して Rust デコード経路に戻す(`SessionScreen` の `syncDecodeMode`)。
録画停止で対応環境なら自動的に WebCodecs へ復帰する。二経路の切り替えは
次のキーフレームで自己回復するため、ホスト側の変更は不要。

## 影響

- 対応 WebView では IPC 帯域が JPEG 経路の ~8 MB/s から VP9 パケットの
  ~4 MB/s へ低下し、Rust 側の VP9 デコード + JPEG 再エンコードの CPU も消える。
- 非対応 WebView・デコード失敗時・録画中は従来どおり JPEG 経路で確実に動作。
- `yuv-renderer.ts` は引き続き凍結(削除は WebCodecs 経路が全対象環境で
  実運用検証された後)。
- **未検証**: Rust 側はサンドボックスの crates.io 遮断によりコンパイル未検証。
  マージ前に `cargo test -p miru-client` と実機での VP9 セッション
  (WebCodecs 描画・エラー/録画フォールバック)を確認すること。
