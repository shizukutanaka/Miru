# ADR 0015: リレープロトコルは MessagePack を使う (JSON ではない)

**状態**: 承認 (2026-06-13)

## コンテキスト

`miru-transport/src/relay.rs` の `send_msg` は `serde_json::to_vec(&msg)` で
メッセージをシリアライズしていた。コメント ("MessagePack over QUIC/WebSocket") は
正しい意図を示していたが、実装は JSON だった。

`Msg::VideoFrame` の `data: Vec<u8>` (VP9/JPEG ペイロード) を serde_json で
シリアライズすると、バイナリデータが **整数配列** `[0, 255, 128, …]` になる。

## 問題: JSON 整数配列の 3× 膨張

各バイトが 1–3 文字の数字 + カンマに変換される (平均 ~3 文字)。

| フレーム種別        | バイナリ  | JSON シリアライズ後 | 比率  |
|---------------------|-----------|---------------------|-------|
| VP9 P フレーム (30KB) | 30,000 B | ~90,000 B           | 3×    |
| VP9 I フレーム (150KB)| 150,000 B| ~450,000 B          | 3×    |
| JPEG フレーム (100KB) | 100,000 B| ~300,000 B          | 3×    |

これは暗号化 → 送信される実データが 3 倍になることを意味する。
30fps × 30KB P フレームを例にすると:
- JSON 化後: 90KB/frame × 30fps = **2.7 MB/s** のシリアライズ出力
- 実際の VP9 データ: 30KB/frame × 30fps = **0.9 MB/s**

VP9 エンコードが達成した帯域圧縮効果をシリアライゼーション層が 3 倍に膨らませていた。

## 判断

**リレープロトコルのシリアライゼーションを `serde_json` から `rmp-serde` (MessagePack) に変更する。**

加えて、バイナリペイロードを持つ全構造体 (`VideoFrame`, `AudioFrame`,
`ClipboardSync`, `FileTransfer::Chunk`) の `data: Vec<u8>` フィールドに
`#[serde(with = "serde_bytes")]` を付与する。

MessagePack は `serde_bytes` 注釈を受けた `Vec<u8>` を **bytes 型**
(2–5 バイトのヘッダー + 生バイト) として格納するため、オーバーヘッドはほぼゼロ。

## 理由

### MessagePack を選んだ理由

- **serde 互換**: 既存の `#[derive(Serialize, Deserialize)]` をそのまま使用できる。
  `Msg` enum の `#[serde(tag = "type")]` (internally-tagged) も MessagePack map で
  表現できるため、メッセージ定義の変更不要。
- **bytes 型サポート**: `serde_bytes` と組み合わせると VP9/JPEG ペイロードが
  ほぼ 1:1 で格納される。
- **シグナルサーバー不変**: リレーサーバーはペイロードを不透明バイトとして
  転送するだけなのでシグナルサーバーコードの変更不要。
- **bincode/postcard 不採用理由**: internally-tagged enum (`#[serde(tag = "type")]`) を
  サポートしない。`Msg` 定義の変更が必要になる。

### 他の選択肢を不採用にした理由

- **serde_bytes のみ (JSON のまま)**: serde_json は `serialize_bytes` 呼び出しを
  受けても整数配列を出力する。`serde_bytes` + serde_json は効果なし。
- **base64 カスタムシリアライザ**: 1/3 改善 (3× → 1.33×) だが、MessagePack の
  ~1× に劣る。実装複雑度も高い。
- **WebSocket バイナリフレーム直送**: `Msg` 型の variant dispatch が失われ、
  フレーミング層の再実装が必要になる。

## 効果

テスト (`msgpack_is_smaller_than_json_for_video_frames`) で検証済み:
- 30KB VP9 フレームの `Msg::VideoFrame` をシリアライズした場合、
  MessagePack サイズが JSON の **40% 未満**。

実効スループット改善:
- シリアライズ出力: 2.7 MB/s → ~1.0 MB/s (30fps × 30KB P フレーム)
- ChaCha20-Poly1305 の暗号化データ量が同比率で削減される

## 破壊的変更

ホスト・ビューア・MCP ブリッジは同一リポジトリ内で同時にアップデートされるため、
既存セッションとの非互換性は問題にならない。外部クライアントが存在する場合は
`/relay` エンドポイントのバージョニング (e.g. `?proto=2`) を検討すること。
