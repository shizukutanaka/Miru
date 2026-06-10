# Miru プロトコル仕様 v1

## 概要

Miru はテキストベース（JSON）の制御メッセージ + バイナリ（暗号化）のメディアフレームを使用する。
全データは X25519+ChaCha20-Poly1305 で E2E 暗号化される。

## フレーミング

すべての TCP/WebSocket フレームは:
```
[4-byte LE length] [encrypted payload]
encrypted payload = [8-byte LE seq nonce] [ciphertext+tag]
```

QUIC ストリームではストリーム自体がフレーム境界を提供するため不要。

## メッセージ型 (JSON)

### 共通フィールド
- `type`: 文字列、メッセージ識別子 (snake_case)
- 残りのフィールドは type 固有

### Hello (Viewer→Host)
```json
{
  "type": "hello",
  "version": 1,
  "role": "viewer",
  "pubkey": "<base64 X25519 public key>",
  "features": {
    "codecs": ["av1", "h265", "h264", "vp9"],
    "audio_codecs": ["opus"],
    "hw_encode": false,
    "hw_decode": true,
    "clipboard": true,
    "file_transfer": true,
    "audio": true,
    "multi_monitor": true
  }
}
```

### HelloAck (Host→Viewer)
```json
{
  "type": "hello_ack",
  "session_id": "<UUID>",
  "pubkey": "<base64 X25519 public key>",
  "encrypted_key": [],
  "selected_codec": "av1",
  "selected_audio": "opus"
}
```
両者は `X25519(own_secret, peer_pubkey)` で 32-byte 共有鍵を導出。
以降のメッセージはすべて `SessionCipher` で暗号化される。

### VideoFrame
```json
{
  "type": "video_frame",
  "seq": 12345,
  "display_idx": 0,
  "keyframe": false,
  "codec": "vp9",
  "data": [<bytes>],
  "width": 1920,
  "height": 1080,
  "timestamp_ms": 1727654321000
}
```

### InputEvent
```json
{
  "type": "input_event",
  "kind": {
    "k": "mouse_move",
    "x": 0.5,
    "y": 0.5,
    "display": 0
  },
  "timestamp_ms": 1727654321000
}
```

座標は全て **0.0–1.0 正規化** (ディスプレイ解像度非依存)。

### QosUpdate (Host or Viewer)
```json
{
  "type": "qos_update",
  "fps": 30,
  "bitrate_kbps": 3000,
  "quality": 75
}
```

### Ping/Pong
```json
{ "type": "ping", "ts": 1727654321000 }
{ "type": "pong", "ts": 1727654321000, "server_ts": 1727654321010 }
```

RTT 計測: `now - ping.ts`

## 接続フェーズ

```
1. Discovery
   Both sides connect to signal server (rendezvous)
   Both register their device_id

2. Initiation
   Viewer sends Connect{target_id} to signal
   Signal forwards RelayOffer to host with token
   Signal returns RelayOffer to viewer with same token

3. Relay or P2P
   Both connect to relay/<token>
   OR: attempt QUIC hole-punch (UDP)

4. Handshake
   Viewer → Host : Hello
   Host → Viewer : HelloAck
   Both derive shared key via X25519

5. Streaming
   Host → Viewer : VideoFrame*, AudioFrame*
   Viewer → Host : InputEvent*, ClipboardSync*
   Either : Ping/Pong every 1s, QosUpdate as needed

6. Termination
   Either side : Close{code, reason}
```

## エラーコード

| Code | 意味 |
|------|------|
| 1000 | 正常終了 |
| 1001 | プロトコルバージョン非互換 |
| 1002 | 認証失敗 |
| 1003 | 権限不足 |
| 1004 | アイドルタイムアウト |
| 1010 | コーデックネゴシエーション失敗 |
| 1020 | ホスト拒否 |
| 1100 | 内部エラー |

## バージョニング

`PROTOCOL_VERSION` (現在: 1) は破壊的変更時にのみ増加。
追加フィールドは無視されるべき (forward compatibility)。
