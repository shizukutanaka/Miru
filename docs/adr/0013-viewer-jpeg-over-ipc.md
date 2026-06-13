# ADR 0013: ビューアは VP9-over-ネットワーク + JPEG-over-IPC とする

**状態**: 承認 (2026-06-13)

## コンテキスト

ビューアの映像パイプラインは現在こうなっている:

```
ホスト → [VP9] → リレー → ビューア(Rust): VP9 デコード
       → I420 → [JPEG 再エンコード q85] → base64 → emit("video-frame")
       → WebView(JS): JPEG デコード → canvas 描画
```

同一画像が **2 回**コーデック処理される (VP9 デコード → JPEG エンコード →
JPEG デコード)。これを無駄と見て、`crates/miru-client/ui/src/lib/yuv-renderer.ts`
が用意された: 「Rust から生 I420 平面を frontend に渡し、WebGL2 の
フラグメントシェーダで YUV→RGB 変換すれば JPEG 再エンコード/再デコードを
丸ごと省ける」という構想である。

## 判断

**生 I420 を IPC で渡す WebGL2 経路は採用しない。** VP9 をネットワーク経路に、
JPEG を IPC 経路に、それぞれ別目的で使う現行構成を正式採用する。
`yuv-renderer.ts` は将来の WebCodecs 経路 (下記) まで凍結する。

## 理由

### 各コーデックは別々のボトルネックに効いている

- **VP9 = ネットワーク帯域のため**: リレー回線 (クロスインターネット) を
  流れる。VP9 は JPEG より桁違いに帯域効率が良く、ここでは必須。
- **JPEG = IPC 帯域のため**: Rust ⇄ WebView 境界 (Tauri は `emit` を JSON
  シリアライズする) を流れる。生フレームはこの境界には大きすぎる。

### 生 I420-over-IPC は 15 倍の退行になる

1080p で 1 フレームあたり:

| 経路            | サイズ           | 30fps スループット |
|-----------------|------------------|--------------------|
| 生 I420 (base64)| ≈ 4.1 MB         | ≈ **124 MB/s**     |
| JPEG q85 (base64)| ≈ 270 KB        | ≈ **8 MB/s**       |

Tauri のイベントは JSON 経由のためバイナリも base64 化が必要であり、
WebGL2 化は描画コストを下げる代わりに IPC 帯域を **約 15 倍**に悪化させる。
真のボトルネックは描画ではなく IPC 帯域なので、`yuv-renderer.ts` は
誤った前提に基づく早すぎる最適化だった。

### では何を改善するか (本 ADR と同時に実施)

二重コーデックを消すのではなく、現行経路の中で効く点を改善する:

1. **frontend デコードを `createImageBitmap` に**: `Image` + data-URL は
   メインスレッドでデコードし GC 圧を生む。`createImageBitmap(Blob)` は
   メインスレッド外でデコードし、描画ジャンクを減らす。
2. **最新フレーム優先ドロップ**: デコードは単一実行 (single-flight) とし、
   デコード中に届いたフレームは最新 1 枚だけ保持する。CPU 逼迫時に
   バックログを溜めず、操作遅延の増大を防ぐ。

## 将来 — WebCodecs 経路で二重コーデックを正しく消す

二重コーデックを真に消す正攻法は WebGL ではなく **WebCodecs `VideoDecoder`**:

```
ホスト → [VP9] → リレー → ビューア(Rust): デコードせず VP9 パケット
       (≈ 50–150KB, 小さい) を base64 → emit
       → WebView(JS): VideoDecoder で VP9 デコード → VideoFrame を直接描画
```

これなら Rust 側の VP9 デコードも JPEG 再エンコードも消え、IPC には
小さい VP9 パケットだけが流れる (≈ 4 MB/s、JPEG より更に低帯域)。
ただし WebView の WebCodecs 対応に依存する (Linux の WebKitGTK は
VP9 対応が限定的)。対応状況が揃った時点で `yuv-renderer.ts` は破棄し、
本経路へ移行する。

## 影響

- 現行 JPEG-over-IPC を維持 (退行リスクを回避)。
- frontend デコードの改善を即時適用。
- `yuv-renderer.ts` はヘッダで本 ADR を参照し「凍結」と明示。
