# ADR 0002: QUIC P2P を主、WebSocket リレーをフォールバック

**状態**: 承認 (2026-04-29)

## コンテキスト

トランスポートプロトコル選定。NAT 環境下のクロスインターネット接続が主。

## 判断

1. **第一候補**: QUIC (Quinn) で UDP hole-punch P2P 接続
2. **フォールバック**: WebSocket (over TCP/443) リレー経由

## 理由

### QUIC を P2P に選ぶ理由
- UDP ベース → NAT hole punching が成立しやすい
- 多重化ストリーム → 動画/音声/制御を独立ストリームで送信、HoL blocking 回避
- 0-RTT 再接続 → 短時間切断時の再ハンドシェイクなし
- 内蔵暗号 (TLS 1.3) → 別途 TLS スタック不要
- パケットロス耐性 → TCP より動画ストリーム向き

### WebSocket (TCP/443) をフォールバックにする理由
- 企業ファイアウォールは TCP/443 のみ開放のことが多い
- QUIC 不通の環境 (carrier-grade NAT、symmetric NAT) でも到達可能
- ブラウザクライアント (将来) との互換性
- リレーサーバーは E2E 暗号化されているため平文を見ない

### WebRTC を採用しなかった理由
- DataChannel は UDP/SCTP のオーバーヘッド大
- 実装が複雑 (SDP, ICE, DTLS, SRTP)
- Rust の純実装が成熟途上 (str0m はあるが小規模)
- ブラウザ要件がない場合は QUIC の方がシンプル

## 結果

- ✓ 多くの環境で P2P 直接接続成立
- ✓ 困難な環境でもリレーで動作保証
- ✓ プロトコルスタック単純化 (QUIC が TLS+多重化を内蔵)
- ✗ ブラウザクライアントは別途 WebRTC bridge が必要 (v1.0 で検討)
