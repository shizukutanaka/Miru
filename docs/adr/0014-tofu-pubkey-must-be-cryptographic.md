# ADR 0014: TOFU の pubkey_b64 は暗号的公開鍵でなければならない

**状態**: 承認 (2026-06-13)

## コンテキスト

`miru-auth` の `AclStore` は TOFU (Trust On First Use) モデルを実装する:
初回接続を自動受け入れし、以降は `pubkey_b64` が一致するか検証する。

`miru-host/src/session.rs` の `check_or_pair()` は `AclStore::check(device_id, pubkey_b64)` を
呼ぶが、**実装バグ**として `pubkey_b64` に Ed25519 公開鍵バイトではなく `device_id` 文字列を
渡していた:

```rust
// バグのあったコード
match acl.check(device_str, device_str) {          // 第2引数が device_str
    TrustDecision::Unknown => {
        acl.trust(TrustedPeer {
            pubkey_b64: device_str.to_string(),    // pubkey として device_id を保存
            ...
        });
    }
}
```

ハンドシェイクは `peer_identity_pubkey: [u8; 32]` (Ed25519 公開鍵) を正しく
返していたが、それは指紋表示にしか使われていなかった。

## 問題: TOFU 鍵固定の完全無効化

`AclStore::check` の比較は `peer.pubkey_b64 == 引数の pubkey_b64`。
保存値も引数も `device_str` なので常に等しく、`TrustDecision::PubkeyMismatch` が
**発火する条件が存在しなかった**。

### 攻撃シナリオ

1. Alice が Bob のホストに接続 → TOFU 受け入れ。ACL に `{device_id: "abc", pubkey_b64: "abc"}` 保存。
2. Mallory がシグナル通信を傍受して Alice の `device_id = "abc"` を入手。
3. Mallory が自分の Ed25519 鍵で `device_id = "abc"` を名乗って接続。
4. ハンドシェイクは Mallory 自身の鍵で通る (自分の署名を正しく使う)。
5. `check_or_pair("abc", "abc")` → `"abc" == "abc"` → `Trusted(Control)` → **全制御権取得**。

デバイス ID はシグナルプロトコル上で平文であるため、パッシブなオブザーバーでも
信頼済みデバイス ID を収集できる。

## 判断

**`pubkey_b64` には常にハンドシェイクが返した Ed25519 公開鍵の Base64 を使う。**

```rust
// 修正後
let pubkey_b64 = B64.encode(pubkey);                // 実際の暗号鍵を base64 化
match acl.check(device_str, &pubkey_b64) {
    TrustDecision::Unknown => {
        acl.trust(TrustedPeer {
            pubkey_b64,                              // 暗号鍵を保存
            ...
        });
    }
}
```

## 影響

### セキュリティ
- `TrustDecision::PubkeyMismatch` が正しく機能するようになり、
  デバイス ID 詐称による不正接続を防ぐ。
- 既存の `acl.json` との互換性は失われる (保存値が device_id 文字列のため
  全エントリが PubkeyMismatch になる)。v0.1 プレリリースでは受け入れられる。
  v0.3 のペアリング UI リリース時に移行ガイドを提供する。

### テスト
`acl_rejects_device_id_impersonation` テストを追加:
- Alice と Mallory の実際の Ed25519 鍵ペアを生成
- Alice の pubkey_b64 (実際の鍵) で ACL に登録
- Mallory が Alice の device_id を名乗ると PubkeyMismatch を返すことを確認
- device_id 文字列そのものを pubkey_b64 として渡すことも拒否することを確認

## 教訓

`AclStore::check(device_id, pubkey_b64)` のシグネチャは型が `(&str, &str)` のため、
引数を取り違えてもコンパイルエラーにならない。型付きラッパー (e.g. `struct PubkeyB64(String)`)
の導入を検討すること。
