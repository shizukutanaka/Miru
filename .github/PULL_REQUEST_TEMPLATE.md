<!-- タイトルは Conventional Commits 形式で: feat(scope):, fix(scope):, docs(scope):, refactor(scope): -->

## 何を / What

<!-- このPRが何を変えるかを1-2文で -->

## なぜ / Why

<!-- どんな問題を解決するか。関連 Issue があれば: Closes #123 -->

## どう / How

<!-- 実装の核となる判断や、トレードオフを書いてください -->

## チェックリスト

- [ ] `cargo fmt` 通過
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 通過
- [ ] `cargo test --workspace` 通過
- [ ] フロント変更時: `cd crates/miru-client/ui && npm run typecheck`
- [ ] 新規依存があれば理由をコメント
- [ ] パブリック API 変更があれば doctest 更新
- [ ] アーキテクチャ判断なら ADR 追加 (`docs/adr/`)
- [ ] CHANGELOG.md に Unreleased エントリ追加

## スクリーンショット / 動画 (UI 変更時)

<!-- before/after を添付 -->

## 影響範囲

<!-- どのクレート/コンポーネントが影響を受けるか -->

## セキュリティへの影響

<!-- 暗号・認証・入力検証に触れる場合は明記。範囲外なら "なし" -->
