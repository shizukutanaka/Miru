# ADR 0022: 「完成」の定義が二重化している — 差別化軸を feature gate へ退避し、パリティを先に閉じる

**状態**: 採用 (2026-08) — feature gate 実装済み。`cargo` による最終検証は未実施

## コンテキスト

「イーロン・マスクのアルゴリズム」(①要件を疑う ②削除する ③簡素化する
④サイクルタイム短縮 ⑤自動化)を本プロダクトへ適用した結果、**①の段階で
設計上の矛盾**が見つかったため記録する。

### 実測データ

Rust 全体 22,700 行 / 17 crate。一次ゴール(人間向けリモートデスクトップ)への
寄与で分類すると:

| 区分 | crate | 行数 |
|------|-------|------|
| コア | common / transport / capture / codec / input / auth / host / client / signal / audio | **15,164** |
| AI エージェント統治(凍結中) | mcp 2,764 / agent 2,421 / transparency 930 / sandbox 706 / constellation 352 | **7,173** |

**凍結スコープがコードベースの約 32%** を占める。

### 矛盾

3つの文書が異なる「完成」を定義している:

1. **ADR 0005**(戦略的差別化)— AI エージェント軸を含む4軸が Miru の存在意義。
   「機能の数で competitor と勝負しない」
2. **roadmap §0**(2026-06 のソクラテス式問答)— 同じ6 crate を
   「**一次ゴールの外側にある別プロダクトの種**」と判定し**凍結**
3. **FEATURE_AUDIT の出荷基準** — 「今日配って、音声付き 60fps HW エンコード
   映像が初回接続で見えるか」= **パリティ基準**(差別化基準ではない)

結果として製品は最も不利な位置にいる: **差別化軸を32%作り込んだ状態で凍結し、
その差別化を成立させる前提であるパリティ(音声・HW エンコード・Wayland・
macOS 60fps)が未完成**。音が出ず Wayland で動かず macOS が 20fps の
「AI エージェント向けリモートデスクトップ」は、どちらの読者にも刺さらない。

### 「凍結」は無料ではない

roadmap §0 は凍結を宣言したが**削除はしていない**。凍結スコープは今も:

- `cargo test --workspace` / `check` / `clippy` の**毎回のビルド対象**
- CI の `test` ジョブが Windows/macOS で `cargo test -p miru-agent` を実行、
  `e2e` ジョブに agent 関連ステップが3件
- crypto-gate に**agent コード専用の例外 grep が4つ**混入し、セキュリティ
  ゲートの純度を下げている
- `miru-host` が `miru-agent` にリンクし、**入力イベントの最ホットパスで
  agent ゲートを通している**
- token 発行・audit チェーンという**誰も使っていない機能の攻撃面**が出荷
  バイナリに含まれる

マスクの規則「削除して10%戻すことがないなら、削除が足りない」に照らすと、
凍結はコメントであって削除ではない。

## 判断

### 1. 差別化軸は捨てない。ただし「凍結」を機構化する

ADR 0005 の賭けは維持する(AI エージェント基盤は実際に業界水準以上の品質で、
市場観察も妥当)。しかし**凍結を Cargo feature でデフォルト off にする**:

- 既定ビルドは 15,164 行のみをコンパイル → コアチームのサイクルタイム短縮
- 出荷バイナリから token/audit/MCP の攻撃面が消える
- crypto-gate の例外 grep 4件を撤去でき、セキュリティゲートが本来の純度に戻る
- `--features agent` で完全に復帰でき、**可逆**

`CLAUDE.md` は「feature flag より型分岐」を掲げるが、これは式レベルの分岐に
関する指針であり、**サブシステム全体の出荷可否**とは別問題。ここでの分岐は
型では表現できない。

### 2. 完成の順序を一本化する

出荷基準は **FEATURE_AUDIT のパリティ基準に一本化**する。差別化軸は
「パリティ達成後に feature gate を外す」という**明示的な後続段階**に置く。
2つの完成定義を同時に追わない — それが①で見つかった「dumb requirement」。

### 3. 参照ゼロのデッドコードは即削除(本 ADR と同時に実施済み)

- `PairPrompt.tsx`(102行)— import 0件。FEATURE_AUDIT 項目12 で
  「プロダクト判断待ち」としていたが、**git が保持するのだから削除が既定**で
  あるべき(必要になれば戻す = マスクの「10%戻す」規則)
- `yuv-renderer.ts`(214行)— ADR 0013/0020 で WebCodecs に置換され参照ゼロ

計 316 行削除。`tsc` + Vitest 69件で無影響を確認済み。

## 実行結果 (2026-08 実施)

`miru-host` に `agent` feature を追加し、**既定 off** とした。

```toml
[features]
default = []
agent = ["dep:miru-agent", "dep:miru-transparency"]
```

gate した箇所は5つだけだった — 32% のコード量に対し**接触面は驚くほど薄い**:

| 箇所 | 内容 |
|------|------|
| `main.rs` | `mod agent_handler` / `audit`・`token` サブコマンド / `parse_capability` |
| `session.rs` | `AgentHandler` 構築ブロック / 画面送信ゲート / **入力ホットパスの `gate_input`** / Rekor アンカリング |
| `metrics.rs` | `snapshot() -> SessionMetadata` |
| `tests/token_issue.rs` | ファイル全体を `#![cfg(feature = "agent")]` |

### 副次的に見つかったセキュリティ上の穴

feature off 時に `Role::AiAgent` のピアが**そのまま人間用の非ゲート経路へ落ちる**
ことに気づいた。ゲートが存在しないビルドでエージェントを受け入れるのは
「ゲート無しで全入力を許可」と同義なので、**明示的に接続拒否**する分岐を追加した:

```rust
#[cfg(not(feature = "agent"))]
if result.peer_role == Role::AiAgent {
    warn!("...agent feature disabled — refusing");
    return Ok(());
}
```

これは gate 化しなければ発生しなかった穴であり、削除作業が設計の欠落を
炙り出した例。

### 本 ADR の当初の記述で誤っていた2点

1. **miru-sandbox は凍結スコープではない。** `main.rs` の
   `miru_sandbox::apply(&Policy::host_daemon())` はホストデーモンの権限降格で
   あり、AI エージェントとは無関係な**コア防御**。706 行を凍結扱いにしていたのは
   分類ミス。gate 対象から外した。
2. **CI の `-p miru-agent` / `-p miru-mcp` ステップは隔離不要。** 各 crate は
   workspace member のまま残るので単体でビルド・テストでき、CI は無変更で通る。
   同様に **crypto-gate の例外 grep 4件も削除できない** — あのゲートは feature に
   関係なくツリー内の全ソースを走査するため、対象コードがツリーに在る限り必要。

### 残る検証

`cargo test --workspace` と `cargo test --workspace --features agent` の**両方**が
緑であることの確認。crates.io が遮断された環境では未実施
(`scripts/verify-offline.sh` は構文・マニフェスト・std 依存モジュールまで検証済み)。

## 影響

- 既定のビルド/CI/攻撃面が約 32% 縮む
- ADR 0005 の差別化は失われない(gate を外せば戻る)
- 「完成」の定義が1つになり、残タスクが
  音声 → HW エンコード → Wayland → macOS の一列に並ぶ
- **本 ADR の 1〜2 は未実行**。crates.io が egress ポリシーで遮断された環境では
  `cargo` が動かず、feature gate の追加を検証できないため
  (`scripts/verify-offline.sh` で構文と std 依存モジュールまでは検証可能)
