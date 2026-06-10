# ADR-0008: 方向別 AEAD 鍵分割 (miru-h2v-v1 / miru-v2h-v1)

- Status: Accepted
- Date: 2026-05
- Deciders: shizukutanaka

## Context

当初の実装では単一の session key を用いて host と viewer 双方が暗号化していた。
これにより:

1. **Nonce 衝突**: 両端が seq=0 からカウントを開始するため、同一 key + 同一 nonce で
   暗号化が発生。ChaCha20-Poly1305 の nonce reuse = keystream 再利用 = 機密性消失。

2. **`SessionCipher::Clone` の危険**: Clone は send_nonce を複製し、同じ nonce を再使用する。
   これも keystream 再利用に直結。

Sprint 8.5 のコード監査 (Issue 22, 22b, 25) でこれらを発見。

## Decision

**HKDF-SHA256 で shared secret から方向別 subkey を派生させる**:

```
host → viewer 方向: HKDF(session_key, info="miru-h2v-v1")
viewer → host 方向: HKDF(session_key, info="miru-v2h-v1")
```

- Host の `tx_cipher` は `miru-h2v-v1` 派生、Viewer の `rx_cipher` も `miru-h2v-v1` 派生
- 両端の send counter は独立、nonce 衝突は構造的に不可能
- `SessionCipher` は `Clone` を実装しない (コンパイルエラーで enfore)
- CI gate が `derive.*Clone.*SessionCipher` を検出して fail

HKDF info string は versioned (`-v1` suffix) で将来の protocol upgrade に対応。

## Consequences

**正**: Nonce reuse クラスのバグを構造的に排除。両端が独立した Counter を持つため、
接続が長寿命化しても衝突なし。

**負**: handshake 後に `HandshakeResult.cipher` (単一) から `HandshakeResult { tx, rx }` への
API 変更が必要 → relay.rs, connection.rs, quic.rs, session.rs すべての消費箇所を更新した。

**ルール**: `derive_subkey(info)` 経由以外での cipher 複製は禁止。ストリーム分割が
必要な場合は `cipher.derive_subkey(b"my-stream-v1")` を使用すること。
