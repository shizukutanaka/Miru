# Miru 改善計画

> 2026-06 更新: 詳細な長所短所レビューと改善記録は
> [`docs/PRODUCT_REVIEW.md`](docs/PRODUCT_REVIEW.md) を参照。

## 長所

- [x] Rust + Tauri — メモリ安全 + 軽量デスクトップ
- [x] 16 crate 分離 — Carmack/Martin/Pike 流の責務分離
- [x] 4軸独自性 — AI Agent / Verifiable E2E / Constellation / Temporal
- [x] AEAD per-direction subkey + 128-bit replay window
- [x] Ed25519 全 verify_strict (ZIP-215) — CI gate で再発不能
- [x] 監査ログ redaction matrix — 強制 (append で bypass 不可)
- [x] MCP rate limit 12 cap + parent process allowlist + TTL ≤15分
- [x] TOFU tamper-evident fingerprint log (HMAC chain)
- [x] Token revocation list (panic rotation)
- [x] 84+ tests 全緑 (5 security crates)
- [x] THREAT_MODEL / CRYPTO / INCIDENT_RESPONSE / SECURITY 4 doc 完備
- [x] cargo-deny + CI crypto gate
- [x] README 4軸独自性を明確に表現

## 短所 → 改善タスク

### P0: リリースブロッカー

- [x] 1. vpx-sys optional 化 → workspace 全体 `cargo check` 通過
- [ ] 2. Tauri CSP 強化 (script-src, frame-src, object-src, isolation, freezePrototype)
- [ ] 3. Tauri capabilities dir 作成 (per-window 最小権限)
- [x] 4. cargo + clippy 全クレート通過 (fmt / clippy 0 warnings, 173 tests — 2026-06)
- [x] 5. README セキュリティセクション更新 (173 tests, verify_strict, redaction — 2026-06)

### P1: 品質ゲート

- [ ] 6. production unwrap triage (136件 → 安全理由コメント or Result 変換)
- [ ] 7. Merkle proof バグ修正 (transparency)
- [ ] 8. miru-codec VP9 encoder を vpx crate v0.4+ へ移行 or pure-Rust fallback
- [ ] 9. 15 TODO をコメント分類 (v0.1 必須 / v1.0 / v2.0)

### P2: v1.0 準備

- [ ] 10. miru-sandbox crate (landlock + seccomp scaffolding)
- [ ] 11. host_bridge 実装 (現 stub → 実 capture pipeline)
- [ ] 12. ADR (Architecture Decision Records) 追加
