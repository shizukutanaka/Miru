# Miru の独自性 — 「他にはない4つの軸」

> リモートデスクトップを「人間 ↔ 人間の画面共有」から「全デバイスと全エージェントが
> 安全に協調する fabric」へ。

## 軸1: AI エージェント First Class

### 課題
Anthropic Computer Use, Claude Code, OpenAI Operator, Adept ACT-1 等、AI が PC を
操作する製品が急増。だが各社が独自に画面キャプチャ + 入力注入を再実装し、
セキュリティモデルもバラバラ。**汎用かつ安全な基盤がない**。

### Miru の解
プロトコル層から AI エージェントを `Role::AiAgent` として組み込む:

| 機能 | 実装 |
|------|------|
| Capability-scoped tokens | Ed25519 署名付きトークン (JWT より高速・小さい・安全) |
| 11個の細粒度権限 | ScreenRead / PointerMove / KeyType / ShellExec 他 |
| 危険操作の自動確認要請 | ShellExec / FileWrite / ClipboardRead は人間確認必須 |
| 自動承認キャッシュ | 同一スコープは30秒間記憶 → 連打プロンプト回避 |
| 改ざん検知付き audit log | SHA-256 chain で全アクションを永続記録 |
| 時限式セッション | デフォルト1時間、自動失効 |

### 使用例 (Claude Code から)
```rust
// Host が UI で「AI に許可」ボタン押下 → トークン発行
let token = AgentToken::issue(
    &host_signing_key,
    "claude-code-session",
    Capability::assistant_default(),  // 最小権限デフォルト
    Duration::from_secs(3600),
    None,
);
// トークン文字列 (JWT 風コンパクト形式) を AI に渡す
println!("MIRU_AGENT_TOKEN={}", token.to_string());

// AI 側: トークンを presenting して接続
// Host 側: 各アクションを authorize() ゲートで判定
session.authorize(Capability::PointerClick, json!({"x":100,"y":200}), "click")?;
```

### 競合との差
- **RustDesk/Parsec/AnyDesk/TeamViewer**: AI エージェント概念なし
- **Anthropic Computer Use**: 認可モデルなし、人間操作と区別なし
- **Miru**: 設計の初日から組み込み

## 軸2: Verifiable End-to-End

### 課題
全競合が「E2E 暗号化です」と主張する。でも**証明できない**。
リレーが本当に何も見ていないか、利用者は確認手段がない。

### Miru の解
セッションごとにメタデータの暗号学的コミットメント:

1. セッション中、両端は metadata (session_id, pubkeys, frame count, byte count) を蓄積
2. セッション終了時、`commitment = SHA256(metadata || nonce)` を計算
3. 双方が commitment に Ed25519 署名 → `CoSignedCommitment`
4. ローカル Merkle log に追記
5. (オプション) Sigstore Rekor 互換の透明性ログに root を投稿

### 検証可能性
```rust
// 6か月後、利用者が「あのセッションは改ざんされていない」と証明する
let metadata = recover_from_local_log();
let commitment = recover_from_local_log();
commitment.verify(&metadata)?;  // ← OK なら、host も viewer も同じデータに合意していた

// Merkle proof で「私のセッションがログに含まれていた」も証明可能
let proof = log.proof(my_session_index)?;
verify_proof(&my_leaf, &proof, my_session_index, &published_root); // → true
```

### 想定攻撃
- リレーがセッション内容を漏洩 → metadata の整合性は守られる (内容は最初から暗号化)
- リレーが「そんなセッションなかった」と否認 → ログに含まれていることを Merkle proof で証明
- ホスト or ビューアの片方が事後改ざん → 両者の署名が必要なので不可能

### 競合との差
- 全競合: 主張のみ
- Miru: 数学的に検証可能

## 軸3: Device Constellation

### 課題
ノートPC、デスクトップ、スマホ、サーバー — 利用者は複数デバイスを持つが、
全競合が **1:1 接続前提**。複数デバイスをひとつの fabric として扱えない。

### Miru の解
Constellation = 利用者の全デバイスを束ねる Ed25519 署名グラフ:

| 概念 | 動作 |
|------|------|
| Constellation key | Ed25519 keypair。利用者本人を表す |
| Device join | Constellation key で署名された device pubkey 証明 |
| Capability advertisement | 各デバイスが解像度・mic・battery・status を自己申告 |
| Hub | 現在のコーディネータ (最も安定したデバイス、自動選出) |
| Handoff | アクティブセッションをデバイス間で移譲 (Apple Continuity 風 + クロス OS) |

### 使用例
```
1. 朝、デスクトップで Slack を開く
2. 出かけるためスマホを取り出す → 自動的に「Desktop で active 中」を検出
3. 「ハンドオフ」タップ → ノートPC(車内) でセッション継続
4. オフィスで会議室の大画面 → 同じセッションが移動
```

### 競合との差
- **Apple Handoff**: 同じ Apple ID 必須、Apple OS 限定
- **Spotify Connect**: 音楽再生のみ、コンピュータ制御なし
- **RustDesk/Parsec**: 1:1 のみ、デバイス複数管理なし
- **Miru**: クロス OS、暗号学的署名、コンピュータ制御

## 軸4: Temporal (Time Travel)

### 課題
全競合がリアルタイムのみ。「昨日のあの操作は何だった?」「先週の障害発生時の画面を
見たい」を満たす製品がない (録画はあっても scrub できない)。

### Miru の解
すべてのセッションを deterministic に記録、UI でスクラブ・倍速・特定時刻ジャンプ:

| 機能 | 実装状態 |
|------|---------|
| 動画記録 (codec passthrough) | ✓ `recording.rs` |
| 入力イベントタイムライン | 計画 v0.3 |
| Scrub UI (Premiere 風) | 計画 v0.3 |
| 倍速再生 (0.25x〜8x) | 計画 v0.3 |
| ブックマーク (`Ctrl+B`でマーキング) | 計画 v0.4 |
| 入力イベントリプレイ (deterministic) | 計画 v1.0 |

### 用途
- **監査**: 規制業界で「あのトランザクションを誰が実行したか」を可視化
- **教育**: 操作の録画を後から見ながら学習
- **デバッグ**: 「この設定変更がいつ行われたか」をピンポイント特定
- **証拠保全**: ハラスメント等の証拠

### 競合との差
- **Parsec/AnyDesk Enterprise**: 録画あり、scrub なし
- **TeamViewer**: 同上
- **Miru**: 4D (時間軸) UI 標準

---

## 「他にはない」ポジショニング

```
                    人間の使いやすさ
                          ↑
                          |
           AnyDesk ●──────┼──────● TeamViewer
                          |        (高機能、高価格)
                          |
        Parsec ●          |          ● Sunshine
        (低レイテンシ)     |          (ゲーミング特化)
                          |
                          |          ● RustDesk
                          |          (オープンソース、機能多)
        ──────────────────┼──────────────────→
                          |          検証可能性
                          |
                          ● Miru ★
                          (AI agent + verifiable + constellation)
```

Miru は単なる "remote desktop" ではなく **"AI 時代の安全な PC fabric"**。
Anthropic Computer Use 等の AI エージェントが採用すれば、Miru は infrastructure layer
になる潜在性。
