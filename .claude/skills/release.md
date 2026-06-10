# .claude/skills/release.md
# Miru リリース手順

## バージョンアップ
```bash
# 1. Cargo.toml の version を更新
# 2. CHANGELOG.md に記載
# 3. コミット
git add -A && git commit -m "chore: release v0.x.0"
git tag v0.x.0
git push origin main --tags
# → GitHub Actions が自動で全OS向けバイナリをビルド・リリース
```

## ローカルテスト (全OS)
```bash
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo audit
```

## シグナルサーバーデプロイ
```bash
docker build -t miru-signal -f crates/miru-signal/Dockerfile .
docker push ghcr.io/shizukutanaka/miru-signal:latest
```
