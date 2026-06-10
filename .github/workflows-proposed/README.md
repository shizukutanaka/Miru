# Proposed GitHub Actions workflows

このディレクトリの `ci.yml` / `release.yml` は `.github/workflows/` に
置くべきファイルです。このブランチを push した GitHub App には
`workflows` 権限がなく、ワークフローファイルを直接コミットできない
ため、ここに置いています。

有効化するには:

```bash
git mv .github/workflows-proposed/ci.yml .github/workflows/ci.yml
git mv .github/workflows-proposed/release.yml .github/workflows/release.yml
git rm .github/workflows-proposed/README.md
git commit -m "Enable CI / release workflows"
```

v0.1.0 の zip に入っていたものからの主な修正点はコミット
「Fix CI, release pipeline, and cargo-deny ...」のメッセージと
`docs/PRODUCT_REVIEW.md` を参照してください。
