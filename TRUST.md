# Miru の信頼モデル

> 「信頼してください」と言わずに信頼を構築するためのドキュメント。

## 概要

ソフトウェアのセキュリティは、開発者を信頼するか、コードを信頼するか、
または**両方を検証可能にするか**で決まります。Miru は3つ目を選びました。

## 1. ソースコードの完全公開

すべてのコードが GitHub で公開されています:
- リポジトリ: https://github.com/shizukutanaka/miru
- ライセンス: MIT (商用利用可、改変可、再配布可)
- 隠しモジュール・難読化なし

## 2. 再現可能ビルド

同じコミットから誰でも bit-identical なバイナリを生成できます。

### 検証手順

```bash
# 1. リリースタグをチェックアウト
git clone https://github.com/shizukutanaka/miru
cd miru
git checkout v0.1.0

# 2. 公式と同じビルドコンテナで再ビルド
./scripts/reproducible-build.sh

# 3. 公式リリースと SHA-256 を比較
sha256sum target/release/miru-host
# → 公式 SHA256SUMS と一致するはず
```

不一致の場合は **悪意のあるバイナリ配布の可能性** — ただちに報告してください。

### 環境固定

`scripts/reproducible-build.sh` が固定するもの:
- Rust ツールチェーン (rust-toolchain.toml)
- 依存ライブラリ (Cargo.lock)
- C ライブラリバージョン (build container)
- ロケール (LC_ALL=C)
- タイムスタンプ (SOURCE_DATE_EPOCH)

## 3. 署名済みリリース

すべてのリリースアセットには Ed25519 署名が付きます:

```bash
# SHA256SUMS ファイル取得
curl -L https://github.com/shizukutanaka/miru/releases/v0.1.0/SHA256SUMS -o SHA256SUMS
curl -L https://github.com/shizukutanaka/miru/releases/v0.1.0/SHA256SUMS.sig -o SHA256SUMS.sig

# 公開鍵 (Sigstore Rekor にも登録)
# NOTE: 専用ホスティングドメインは未確保です。公開鍵は GitHub リリースページの
# アセットとして配布してください (miru.app 等の外部ドメインには依存しない)。
curl -L https://github.com/shizukutanaka/miru/releases/download/v0.1.0/release-pubkey.txt -o release-pubkey.txt

# 署名検証
miru-host verify-release SHA256SUMS SHA256SUMS.sig release-pubkey.txt
# → "OK: signed by <fingerprint>, registered in Rekor #<index>"
```

Rekor 登録により、**鍵が将来漏洩しても**過去の署名が偽造されていないことを証明可能。

## 4. 依存関係の透明性

### SBOM (Software Bill of Materials)

各リリースに CycloneDX 形式の SBOM 同梱:
- `miru-0.1.0-sbom.cyclonedx.json`
- 全 Rust crate + バージョン + ライセンス
- 全 C 依存ライブラリ + バージョン

### 自動脆弱性スキャン

- `cargo audit` を CI で必須実行 (warnings → ビルド失敗)
- 月次で全依存の更新可能性チェック
- `cargo deny` でライセンス互換性検証

## 5. 暗号設計の検証可能性

### アルゴリズム

| 用途 | アルゴリズム | 実装 |
|------|------------|------|
| 鍵交換 | X25519 ECDH | x25519-dalek (audited) |
| 対称暗号 | ChaCha20-Poly1305 | chacha20poly1305 (audited) |
| 署名 | Ed25519 | ed25519-dalek (audited) |
| ハッシュ | SHA-256 | ring (audited) |
| KDF | HKDF-SHA256 | ring |
| パスワード | Argon2id | argon2 |

すべてのライブラリは公開監査を経たもののみ採用。**自前暗号実装ゼロ**。

### 監査計画

| 時期 | 対象 | 監査者 |
|------|------|-------|
| v0.5 | 暗号プロトコル | Trail of Bits 候補 |
| v1.0 | 全コードベース | NCC Group / Cure53 候補 |
| 年次 | 差分監査 | 同上 |

監査報告書は **全文公開** (NDAなし)。

## 6. CVE 対策の透明性

RustDesk が抱えた既知の脆弱性に対する Miru の対応:

| RustDesk CVE | 内容 | Miru の対策 |
|---|---|---|
| CVE-2026-30785 | 弱パスワードハッシュ | Argon2id (64MB/3iter/4-way) |
| CVE-2026-30794 | TLS 検証無効フォールバック | `danger_accept_invalid_certs` 関数を提供しない |
| CVE-2026-30791 | 設定の脆弱な暗号化 | identity は平文 (mode 600)、設定は AEAD |
| CVE-2026-30798 | Heartbeat 改ざん | 全制御メッセージ AEAD 暗号化 |
| CVE-2026-2490 | シンボリックリンク漏洩 | O_NOFOLLOW + path 検証 |
| CVE-2024-25140 | テスト証明書出荷 | ビルドプロセスで本番/テスト分離 |
| CVE-2026-30784 | rendezvous/relay の未認証 RegisterPeer 登録による権限昇格 | `miru-signal` の Register ハンドラは Ed25519 所有権証明 (`verify_register_signature`) + identity-lock で他デバイス ID の乗っ取りを拒否。`MIRU_REQUIRE_SIGNED_REGISTER=1` で未署名登録を完全拒否する strict モードあり(デフォルトは後方互換のため警告付き受理、全クライアント移行後に有効化推奨。docs/RESEARCH_NOTES.md §6) |
| CVE-2026-30795 | Heartbeat 同期ループの平文送信 | Miru の全セッションメッセージ (Ping/Pong 含む) は ChaCha20-Poly1305 で暗号化。signal 経由の Register/Connect は device_id/pubkey のみでセッション内容を含まない |

## 7. プライバシー検証

Miru が**何も送信していないこと**を自分で検証する方法:

```bash
# Wireshark / tcpdump で送信パケット監視
# <your-signal-host> は MIRU_SIGNAL に設定した自分の signal サーバーのホスト名/IPに置き換えてください
# (Miru は自己ホスト型で、公式にホストされた既定の signal サーバーは存在しません)
sudo tcpdump -i any host <your-signal-host>

# 接続中以外、何も流れないはずです
```

公式シグナルサーバーへの接続も含めて、すべて
[PRIVACY.md](PRIVACY.md) に記載されたもののみ。

## 8. Bug Bounty (準備中)

v1.0 公開時に Bug Bounty プログラム開始予定:
- Critical: $5,000+
- High: $1,500+
- Medium: $500+
- Low: $100+

報告先: [SECURITY.md](SECURITY.md) 参照(GitHub Private Vulnerability Reporting が
現時点で唯一機能している窓口。`security@miru.app` のメール受付はまだ稼働していません)。

## 9. 外部依存の信頼境界

| 依存 | 信頼レベル | 理由 |
|------|-----------|------|
| 公式 signal/relay | **不要** | 暗号文しか見えない、メタデータも保存しない |
| Sigstore Rekor | **任意** | opt-in、参加しなくても完全動作 |
| GitHub Releases | **必須** | バイナリ配布元、署名で改ざん検知 |
| webpki-roots | **必須** | TLS 検証用 CA、Mozilla 提供 |
| crates.io | **必須** | Cargo.lock + cargo audit で監視 |

## 10. 透明性レポート

四半期ごとに公開予定 (v1.0 以降):
- 受領した法執行機関要請数
- セキュリティパッチリリース数
- 監査結果
- 利用統計 (ただし**収集不可能なため何も書けません**)

## 連絡先

- 技術質問: GitHub Issues
- セキュリティ報告: [SECURITY.md](SECURITY.md) の GitHub Private Vulnerability
  Reporting 経由(`security@miru.app` はまだ稼働していません)
- 監査依頼: 現時点で専用窓口は未設置。GitHub Issues でご連絡ください
  (`trust@miru.app` はまだ稼働していません)
