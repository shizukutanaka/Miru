# Miru — アーキテクチャ設計書

## 概要

P2P優先のリモートデスクトップ。RustDesk/TeamViewer相当の機能を
セルフホスト可能・完全オープンソースで実現する。

---

## 接続フロー

```
Viewer                Signal Server (hbbs)         Host
  |                          |                       |
  |--- Register(viewer_id) ->|                       |
  |                          |<-- Register(host_id) -|
  |                          |                       |
  |--- Connect(host_id) ---->|                       |
  |                          |--- RelayOffer -------->|
  |<-- ConnectAck(relay) ----|                       |
  |                                                  |
  |============ WebSocket Relay (hbbr) =============|
  |                                                  |
  |<------- Hello (X25519 pubkey) ------------------|
  |-------- HelloAck (session key encrypted) ------->|
  |                                                  |
  |  ~~~ QUIC P2P hole-punch attempt ~~~            |
  |  (succeeds → relay dropped, QUIC used)          |
  |                                                  |
  |<======= Encrypted VideoFrame stream ============|
  |======== InputEvent stream =====================>|
```

---

## 各OS スクリーンキャプチャ

### Windows — DXGI Desktop Duplication API
- **API**: `IDXGIOutput1::DuplicateOutput`
- **フォーマット**: BGRA32 または NV12 (GPU texture)
- **dirty rect**: 変更領域のみエンコード対象
- **GPU zero-copy**: DXGI texture → NVENC/QuickSync 直接入力
- **latency**: ~1フレーム (16ms @ 60fps)

### macOS — ScreenCaptureKit (macOS 12.3+)
- **API**: `SCStream` + `SCStreamDelegate`
- **フォーマット**: `IOSurface` (GPU-resident)
- **フォールバック**: `CGDisplayStream` (macOS 10.15+)
- **権限**: `NSScreenCaptureUsageDescription` 必須
- **注意**: macOS 14+ はプライバシー許可ダイアログ毎回

### Linux X11 — MIT-SHM extension
- **API**: `XShmGetImage` (共有メモリ)
- **フォーマット**: BGRA32
- **制限**: Xorg のみ。全画面コピー (dirty rect なし)
- **latency**: ~2フレーム

### Linux Wayland — PipeWire
- **API**: `xdg-desktop-portal` D-Bus → `pw_stream`
- **フォーマット**: DMA-BUF (GPU-resident) または SHM
- **権限**: ユーザー承認ダイアログ (1回)
- **注意**: GNOME 41+ / KDE 5.27+ 必須

---

## コーデックパイプライン

```
RawFrame (BGRA/NV12)
    │
    ▼
ColorConvert (I420 変換 — CPUまたはCUDA)
    │
    ▼
Encoder
  ├─ AV1   (libaom-av1 / SVT-AV1 / hardware)
  ├─ H265  (x265 / NVENC / VideoToolbox / VAAPI)
  ├─ H264  (x264 / NVENC / VideoToolbox / VAAPI)
  └─ VP9   (libvpx — フォールバック)
    │
    ▼
Transport (QUIC stream または WebSocket frame)
    │
    ▼ (viewer側)
Decoder
  ├─ AV1   (dav1d — 最速ソフトウェアデコーダー)
  ├─ H265  (VideoToolbox / NVDEC / VAAPI)
  └─ H264  (ffmpeg)
    │
    ▼
Display (OS native window / WebRTC video)
```

### QoS制御
- RTT監視 → Ping/Pong (1秒間隔)
- packet loss > 5% → bitrate -20%
- RTT > 100ms → fps 30に制限
- RTT > 200ms → fps 15, 解像度 ×0.75

---

## セキュリティ

### 認証
1. デバイスID: 8桁英数字 (例: `A1B2-C3D4`)
2. 接続パスワード: PBKDF2-SHA256 (4桁PIN または任意パスワード)
3. デバイス信頼: Ed25519 公開鍵 (初回接続時TOFU)

### 暗号
- 鍵交換: X25519 ECDH (ephemeral — perfect forward secrecy)
- 対称暗号: ChaCha20-Poly1305 AEAD
- nonce: 64-bit counter (replay attack防止)
- リレーサーバー: 暗号文のみ転送 (平文不可視)

### 認可
- 接続許可: ホスト側UIでAccept/Reject
- 権限レベル: 閲覧のみ / フルコントロール / ファイル転送許可
- セッションタイムアウト: デフォルト8時間

---

## デプロイ

### セルフホスト (Docker Compose)
```yaml
services:
  miru-signal:
    image: ghcr.io/shizukutanaka/miru-signal:latest
    ports:
      - "21115:21115"  # rendezvous
      - "21117:21117"  # relay
    environment:
      - MIRU_LOG=info
    restart: unless-stopped
```

### 環境変数
| 変数 | デフォルト | 説明 |
|------|-----------|------|
| `MIRU_SIGNAL` | `ws://localhost:21115/ws` | シグナルサーバーURL(自己ホスト。既定は開発用ローカルホスト) |
| `MIRU_LOG` | `info` | ログレベル |
| `MIRU_MAX_FPS` | `60` | 最大フレームレート |
| `MIRU_CODEC` | `auto` | 強制コーデック |

---

## 10年間動かすための方針

1. **プロトコルバージョニング**: `PROTOCOL_VERSION` フィールドで後方互換
2. **コーデック抽象**: trait経由 — 将来のAV2等も追加可能
3. **zero external service dependency**: シグナルサーバーはセルフホスト可能
4. **単一バイナリ**: OS依存ライブラリはstaticリンク
5. **定期監査**: `cargo audit` をCI必須に
