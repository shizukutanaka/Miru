# Miru を AI エージェントから使う

Claude Desktop / Claude Code 等の AI エージェントから Miru ホストを安全に制御する手順。

## 全体像

```
┌──────────────────┐    stdio JSON-RPC    ┌─────────────┐    Miru protocol    ┌────────────┐
│  Claude Desktop  │ ──────────────────→ │  miru-mcp   │ ───────────────────→ │  miru-host │
│  (or any LLM)    │                      │  (stdio)    │                       │ (your PC)  │
└──────────────────┘                      └─────────────┘                       └────────────┘
                                                ↑
                                          AgentToken
                                       (capability-scoped,
                                        time-limited)
```

## セットアップ (3ステップ)

### 1. ホスト側: トークン発行

`miru-host` の UI で「AI エージェントを許可」ボタンをクリック。
ダイアログで:
- ラベル (例: `claude-code-session`)
- 有効期限 (1時間 / 4時間 / 8時間 / 24時間)
- 許可する操作 (画面読取、マウス、キーボード等)

を選択して「発行」。表示されるトークン文字列をコピー。

### 2. クライアント側: `miru-mcp` インストール

```bash
# Cargo install (推奨)
cargo install miru-mcp

# または release バイナリを直接ダウンロード
curl -L https://github.com/shizukutanaka/miru/releases/latest/download/miru-mcp-$(uname -m) \
     -o ~/.local/bin/miru-mcp
chmod +x ~/.local/bin/miru-mcp
```

### 3. Claude Desktop 設定

`~/Library/Application Support/Claude/claude_desktop_config.json`
(Linux: `~/.config/Claude/claude_desktop_config.json`)
に追加:

```json
{
  "mcpServers": {
    "miru": {
      "command": "miru-mcp",
      "env": {
        "MIRU_AGENT_TOKEN": "miru-agent.<貼り付け>",
        "MIRU_SIGNAL": "ws://signal.miru.app:21115/ws",
        "MIRU_HOST_DEVICE_ID": "ABCD-1234"
      }
    }
  }
}
```

Claude Desktop を再起動。会話で「Miru で画面キャプチャして」等と話しかけると
ツールが見える状態に。

## 提供ツール

| ツール名 | 機能 | 必要な capability |
|---------|------|------------------|
| `miru_screen_capture` | 画面の PNG 取得 | `screen_read` |
| `miru_mouse_move` | カーソル移動 (正規化座標 0-1) | `pointer_move` |
| `miru_mouse_click` | クリック (左/右/中、ダブル可) | `pointer_click` |
| `miru_scroll` | スクロール | `pointer_move` |
| `miru_key_type` | テキスト入力 | `key_type` |
| `miru_key_combo` | キーボードショートカット | `key_combo` |
| `miru_clipboard_read` | クリップボード読取 | `clipboard_read` (確認必須) |
| `miru_clipboard_write` | クリップボード書込 | `clipboard_write` |
| `miru_open_url` | URL を開く | `open_url` |
| `miru_status` | 接続状況取得 | (なし) |

## セキュリティモデル

1. **最小権限デフォルト** — 危険操作 (`shell_exec`, `file_write`, `clipboard_read`)
   は明示選択しないと付与されない
2. **時限式** — トークンは設定した時間で自動失効、再発行が必要
3. **確認必須操作** — `requires_confirmation` のついた操作は実行ごとに通知 →
   人間が許可しなければ拒否
4. **改ざん検知付き audit log** — すべての AI アクションを SHA-256 chain で
   永続記録、後から監査可能 (`~/.miru/agent-audit.log`)
5. **取消可能** — 信頼済みデバイス一覧から即座に revoke

## audit log の見方

```bash
# Last 10 actions
tail -n 10 ~/.miru/agent-audit.log | jq

# 拒否されたアクション
jq 'select(.outcome == "denied")' ~/.miru/agent-audit.log

# Chain integrity check
miru-host audit verify
# → "OK: 1234 entries, chain intact"
```

## トラブルシュート

**Q: トークンが拒否される**
A: ホストの公開鍵と一致しない署名。トークン発行元のホストに接続しているか確認。

**Q: Claude が「Miru ツールが見えない」と言う**
A: `claude_desktop_config.json` のパスが正しいか、`miru-mcp` コマンドが PATH に
あるか確認。Claude Desktop を完全に再起動。

**Q: 確認ダイアログがブロックされる**
A: v0.1 は CLI モード — `MIRU_AUTO_CONFIRM=safe` で readonly capability のみ
自動承認に。完全 UI 確認は v0.3 で実装。

## 動作確認用ワークフロー (Claude への指示例)

```
「Miru ホストの画面を見て、開いているアプリを教えて」
→ Claude が miru_screen_capture を呼ぶ
→ 画面の PNG が返る
→ Claude が描写

「メモ帳を開いて『Hello』と入力して」
→ Claude が miru_open_url や miru_key_combo を順次呼ぶ
→ 各 capability が authorize() を通過
→ audit log にすべて記録される
```
