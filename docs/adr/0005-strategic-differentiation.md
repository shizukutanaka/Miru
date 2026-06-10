# ADR 0005: 戦略的差別化 — 4つの独自軸

**状態**: 承認 (2026-04-29)

## コンテキスト

リモートデスクトップ市場 (RustDesk, Parsec, Sunshine, AnyDesk, TeamViewer 他) は機能成熟。
単純な「同じものを少し良く」では差別化困難。

調査で発見した未充足ニーズ:

1. **AI エージェント時代の PC 制御基盤がない** — Claude Computer Use 等は急成長だが、すべて
   独自に画面キャプチャ + 入力注入を再実装している。汎用かつ安全な基盤が欠落。
2. **「E2E です」を証明できる競合がない** — 主張のみ。利用者が監査できない。
3. **1:1 接続前提** — 「複数デバイスの constellation」という概念が存在しない。
4. **時間軸操作なし** — すべてリアルタイムのみ。巻き戻し・スクラブ不可。

## 判断

Miru の独自性を以下の **4軸** で確立:

### 軸1: AI Agent First Class
- AI エージェントを「特殊な viewer」として最初から組み込む
- `Role::AiAgent` セッション種別、`AgentToken` による capability-scoped 認可
- 全 AI 操作は audit log に記録、human-in-the-loop 確認モード標準
- MCP (Model Context Protocol) サーバー統合 — Claude/その他 LLM から呼べる
- セッションは時限式 (デフォルト1時間で失効)

### 軸2: Verifiable End-to-End
- 各セッションのメタデータ (session_id, peer fingerprints, frame count, byte count) の
  SHA256 を Merkle tree のリーフに格納
- セッション終了時に両端が Merkle root を相互署名
- root をオプションで Sigstore Rekor 互換の透明性ログに投稿
- 利用者は「自分のセッションが改ざんされていないこと」を後から証明可能

### 軸3: Device Constellation
- 利用者の全デバイス (laptop / phone / desktop / server) を一つの "constellation" として扱う
- 各デバイスが capabilities (display解像度, audio, location, battery, status) を広告
- 中央サーバー不要 (mDNS + signal server federation)
- セッション中にデバイス間でハンドオフ可能 (Spotify "now playing" 風)

### 軸4: Temporal (Time Travel)
- セッション中の全入力イベント + 動画フレームを deterministic に記録
- Replay UI でスクラブ・倍速・特定時刻ジャンプ
- 「昨日の操作を再現」「先週の障害発生時を復元」
- 監査・教育・デバッグ用途

## 結果

- ✓ 競合と「機能の数」で勝負しない、戦略的ポジショニング
- ✓ AI 時代の standard layer になる潜在性 (Anthropic Computer Use の基盤候補)
- ✓ 規制業界 (金融・医療) で透明性ログが差別化要因
- ✓ 検索キーワード「AI 用リモートデスクトップ」で1位を狙える
- ✗ 4軸すべての実装は v0.1 では完成しない、段階的リリース必要
- ✗ AI 軸はエコシステム依存 (MCP 普及度)、ベット要素あり

## 実装優先順位

| 軸 | v0.1 | v0.3 | v1.0 | v2.0 |
|---|------|------|------|------|
| AI Agent | プロトコル + 基本MCPサーバー | スコープ制限+audit UI | 高度なツール | self-hostable AI gateway |
| Verifiable E2E | Merkle root 計算 | local log | Rekor 投稿 | 完全公開検証 |
| Constellation | デバイス広告 | UI 統合 | ハンドオフ | クロスデバイスクリップボード |
| Temporal | 録画API (実装済) | スクラブ UI | 倍速再生 | 入力リプレイ |

## 「やらないこと」

- AI モデルのホスティング — Miru は基盤、モデルは BYO
- ブロックチェーン暗号通貨統合 — 透明性ログは Sigstore 互換のみ
- 中央集権アカウント — 認証は常にデバイス identity ベース
