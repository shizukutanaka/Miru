# RustDesk から Miru への移行ガイド

RustDesk から Miru に乗り換える方向け。

## なぜ移行するか

RustDesk は素晴らしい OSS リモートデスクトップですが、いくつかの設計判断は限界を
迎えつつあります:

| 観点 | RustDesk | Miru |
|------|----------|------|
| **AI エージェント対応** | なし (人間視聴のみ) | ファーストクラス (capability-scoped token) |
| **多デバイス管理** | 1:1 接続のみ | Constellation (N:N fabric) |
| **検証可能 E2E** | 「E2E です」と主張 | 暗号学的に証明可能 (Sigstore Rekor) |
| **セッション巻き戻し** | 録画 (再生のみ) | Premiere風スクラバー (倍速 + ブックマーク) |
| **ライセンス** | AGPLv3 | MIT (商用統合容易) |
| **最近の CVE** | CVE-2026-30785/30794/2490 等 | 設計初日から対策済 |
| **Argon2id 対応** | なし (低 iter PBKDF2) | 採用 (CVE-2026-30785 教訓) |
| **TLS 検証無効フォールバック** | あり (CVE-2026-30794) | 構造的に存在しない |

## 移行で失うもの

正直なところ、現時点で Miru には RustDesk の以下の機能がありません:

- ✗ Web クライアント (v1.0 計画中)
- ✗ モバイルクライアント (v1.0 計画中)
- ✗ Sciter UI (Miru は Tauri のみ)
- ✗ Wayland マルチモニタ完全対応 (PipeWire 経由実装中)
- ✗ TCP ポートフォワード (実装予定なし - WireGuard 推奨)
- ✗ 大規模デプロイメント機能 (Pro Server 等)

これらが必須な場合は v1.0 まで RustDesk を継続推奨します。

## 移行で得るもの

- ✓ AI エージェントから安全に操作される基盤
- ✓ 複数デバイスを統一可視化
- ✓ 監査要件のある業界向け改ざん検知ログ
- ✓ MIT ライセンスで商用統合容易
- ✓ 5.1/7.1 サラウンド音声 (RustDesk はステレオまで)
- ✓ HDR (BT.2020/PQ/HLG) 対応
- ✓ より厳格なセキュリティ設計

## ステップ・バイ・ステップ

### 1. インストール

```bash
# macOS / Linux
brew install shizukutanaka/miru/miru

# Arch
yay -S miru

# Windows
scoop bucket add miru https://github.com/shizukutanaka/scoop-miru
scoop install miru
```

### 2. RustDesk のアンインストール (任意)

両方共存可能です。完全に切り替えたい場合のみ:

| OS | コマンド |
|----|---------|
| macOS | `brew uninstall rustdesk` または `/Applications/RustDesk.app` を削除 |
| Linux | `sudo apt remove rustdesk` または ディストリ標準の方法 |
| Windows | コントロールパネル > プログラムのアンインストール |

### 3. 設定の移行

RustDesk と Miru は **設定ファイルが互換ではありません**。手動で再構築が必要です。

**RustDesk → Miru への移行マッピング**:

| RustDesk | Miru | 備考 |
|----------|------|------|
| ID | デバイス ID | `XXXX-XXXX` 形式に変わります |
| パスワード | 6桁 PIN | 初回ペアリング後は TOFU で代替 |
| カスタムリレー | カスタムシグナル | `MIRU_SIGNAL=ws://...` |
| ホワイトリスト | ACL (信頼済みデバイス) | UI から管理 |
| Permissions | Capability tokens | より細粒度 |

### 4. ホスト設定

```bash
# シンプル接続 (公式シグナル使用)
miru-host

# 自前シグナル
MIRU_SIGNAL=ws://your-signal:21115/ws miru-host

# ヘッドレスサーバー
sudo systemctl enable --now miru-host  # systemd
brew services start miru                # macOS
```

### 5. ビューア設定

Tauri 版の `miru-viewer` を起動 → 初回起動ウィザードを完了 → ホストの ID と
シグナルアドレスを入力。

PIN ペアリング → 指紋照合 → 完了。

### 6. AI 統合 (Miru 独自)

```bash
# UI で「AI 許可」をクリック → トークン発行
# Claude Desktop の設定に追加:
{
  "mcpServers": {
    "miru": {
      "command": "miru-mcp",
      "env": {
        "MIRU_AGENT_TOKEN": "miru-agent.<paste>",
        "MIRU_SIGNAL": "ws://your-signal:21115/ws"
      }
    }
  }
}
```

詳細: [`docs/ai-agent-guide.md`](ai-agent-guide.md)

## よくある質問

### Q: RustDesk のリレーサーバー (HBBS/HBBR) を再利用できますか?

A: **いいえ**。プロトコルが異なります。Miru の signal/relay は別物です。
ただし `docker-compose.yml` 1コマンドで起動できるため、移行は数分で済みます。

### Q: コーデックは何が使えますか?

A: VP9 (実装済) + AV1/H.264/H.265 (HW プローブ済、フル統合は v0.3)。

### Q: パフォーマンスは RustDesk と比べて?

A: v0.1 では RustDesk と同等程度を目標。v1.0 では BBR 系輻輳制御 +
バックプレッシャー + ゼロコピー GPU パイプラインで RustDesk より低レイテンシを
目指します。詳細は [`docs/competitive-analysis.md`](competitive-analysis.md)。

### Q: AGPLv3 ではなく MIT を選んだ理由は?

A: 商用統合を容易にし、Anthropic Computer Use 等の AI 製品が Miru を基盤として
組み込めるようにするためです。AGPL では一部のクラウドサービスが採用しづらい現実が
あります。

### Q: 既存の RustDesk 設定をインポートする CLI ツールはありますか?

A: 現在ありません (v0.3 で `miru-host migrate --from-rustdesk ~/.config/rustdesk`
を計画中)。それまでは手動で再ペアリングしてください。

### Q: 戻りたい場合は?

A: 両方の設定が独立しているため、いつでも RustDesk に戻せます。Miru の
`~/.config/miru/` を削除するだけ。

## サポート

- 質問: GitHub Discussions
- バグ: GitHub Issues
- セキュリティ: [SECURITY.md](../SECURITY.md) 参照(`security@miru.app` はまだ稼働していません)
- 移行で困ったら: GitHub Issues/Discussions へ(専用アドレス `migrate@miru.app` は
  まだ稼働していません)
