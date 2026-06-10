# ADR-0007: Ed25519 verify_strict (ZIP-215) を全署名検証に強制

- Status: Accepted
- Date: 2026-05
- Deciders: shizukutanaka

## Context

Ed25519 には複数の検証実装が存在し、バリデーション基準が diverge している:

- 小順序公開鍵 (cofactor 8 攻撃) を受理するか
- 非正規 R エンコーディングを受理するか
- バッチ検証と単独検証で異なる結果になるか

Henry de Valence の "It's 255:19AM. Do you know what your validation criteria are?" (2020)
が 14 の主要実装で diverge を文書化。NEAR nearcore#2700、IOTA での実例あり。

さらに、ed25519-dalek の非 strict `verify()` は small-order pubkey を受理するケースがあり、
これを悪用すると署名 forger が任意のメッセージに対して valid signature を生成できる
(cofactor 攻撃)。

## Decision

- **全ての production signature verification は `VerifyingKey::verify_strict` を使用する**
- `verify()` (non-strict) の呼び出しは production コードで禁止
- CI の `crypto-gate` ジョブが `\.verify(` を grep し、`_strict` なしなら build fail
- テストは対象外 (test コードは制御下にあるため)

## Consequences

**正**: Small-order pubkey / non-canonical R エンコーディングによる攻撃を構造的に不可能にする。
ZIP-215 準拠により NEAR/Solana 系エコシステムとの相互運用可能性も高まる。

**負**: `verify_strict` は batch verification と組み合わせると signature malleability を
完全に排除するため、将来 batch verification が必要になった場合は慎重な実装が必要。

**副作用**: ed25519-dalek 2.x で `features = ["rand_core"]` が必要 (`rand_core` を
使う `SigningKey::generate()` を使用するため)。
