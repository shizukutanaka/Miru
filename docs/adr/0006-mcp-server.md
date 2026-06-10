# ADR 0006: MCP server as the AI integration surface

**状態**: 承認 (2026-04-29)

## コンテキスト

軸1 (AI Agent First Class) を実現する具体的な技術選択。

候補:
1. **HTTP REST API**: 利用は可能だが、ツール検出 / 認可の標準がない
2. **gRPC**: 性能は良いが、AI ツール側の対応がまばら
3. **Model Context Protocol (MCP)**: Anthropic 公式、Claude Desktop が標準対応、
   stdio + JSON-RPC 2.0 で軽量
4. **OpenAI Function Calling 形式**: ベンダーロックイン、独自規格
5. **OpenAPI / Swagger**: 既存の REST に近いが冗長、リアルタイム性に欠ける

## 判断

**MCP を採用** (`miru-mcp` crate + `miru-mcp` バイナリ)。

理由:
- Claude Desktop / Claude Code がネイティブ対応 (主要ターゲット)
- stdio 通信は単純、サンドボックス容易
- JSON-RPC 2.0 はライブラリ豊富、デバッグ容易
- Anthropic 以外も採用検討中 (OpenAI, Cline 等) — エコシステムの未来あり
- ストリーミング (SSE/HTTP) も標準で規定、将来の Web 統合に対応

## 結果

- ✓ `miru-mcp` バイナリ単独で実行可能、AI 側設定だけで統合
- ✓ 10個の tool (screen_capture, mouse_move, key_combo 等) 実装済み
- ✓ AgentSession と直結 — すべての tool 呼び出しが authorize() ゲートを通る
- ✓ AI 側ロックインなし — どの MCP 対応 LLM からでも使える
- ✗ MCP は仕様策定中、breaking change リスクあり → semver で吸収
- ✗ stdio は IPC のみ (リモートには HTTP transport を後で追加)

## 参考

- 仕様: https://modelcontextprotocol.io/specification
- リファレンス実装: https://github.com/modelcontextprotocol/servers
