import { useState } from "react";
import { api } from "../lib/tauri";

interface Capability {
  id: string;
  label: string;
  danger: boolean;
}

const CAPS: Capability[] = [
  { id: "screen_read", label: "画面を見る", danger: false },
  { id: "pointer_move", label: "マウス移動", danger: false },
  { id: "pointer_click", label: "クリック", danger: false },
  { id: "key_type", label: "テキスト入力", danger: false },
  { id: "key_combo", label: "キーボードショートカット", danger: false },
  { id: "clipboard_write", label: "クリップボード書込", danger: false },
  { id: "open_url", label: "URL を開く", danger: false },
  { id: "clipboard_read", label: "クリップボード読取", danger: true },
  { id: "file_read", label: "ファイル読取", danger: true },
  { id: "file_write", label: "ファイル書込", danger: true },
  { id: "shell_exec", label: "シェルコマンド実行", danger: true },
];

const DEFAULT_CAPS = new Set([
  "screen_read", "pointer_move", "pointer_click",
  "key_type", "key_combo", "clipboard_write", "open_url",
]);

interface Props {
  onIssued: (token: string) => void;
  onCancel: () => void;
}

export function AgentTokenIssue({ onIssued, onCancel }: Props) {
  const [label, setLabel] = useState("claude-code-session");
  const [ttlMins, setTtlMins] = useState(15);
  const [selected, setSelected] = useState<Set<string>>(new Set(DEFAULT_CAPS));
  const [issued, setIssued] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [issuing, setIssuing] = useState(false);

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  const issue = async () => {
    setError(null);
    setIssuing(true);
    try {
      const result = await api.issueAgentToken(
        label.trim(),
        ttlMins,
        Array.from(selected),
      );
      setIssued(result.token);
    } catch (e) {
      setError(String(e));
    } finally {
      setIssuing(false);
    }
  };

  const copyToken = async () => {
    if (!issued) return;
    await navigator.clipboard.writeText(issued);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  if (issued) {
    return (
      <div className="dialog-backdrop">
        <div className="dialog" style={{ maxWidth: 600 }}>
          <h2>トークン発行完了</h2>
          <p className="dialog-subtitle">
            このトークンを Claude Desktop の設定にコピー。漏洩しないよう注意。
          </p>

          <div className="token-display">
            <code>{issued}</code>
          </div>

          <div className="dialog-warning">
            このトークンは {ttlMins} 分で自動失効します。
            紛失しても再発行可能。漏洩した場合は「信頼済みデバイス」から取り消し。
          </div>

          <h3 style={{ marginTop: 20, fontSize: 13 }}>Claude Desktop 設定例</h3>
          <pre className="config-example">{`{
  "mcpServers": {
    "miru": {
      "command": "miru-mcp",
      "env": {
        "MIRU_AGENT_TOKEN": "${issued.substring(0, 40)}…"
      }
    }
  }
}`}</pre>

          <div className="dialog-actions">
            <button className="button-ghost" onClick={() => onIssued(issued)}>閉じる</button>
            <button className="button-primary" onClick={copyToken}>
              {copied ? "コピー済み" : "トークンをコピー"}
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="dialog-backdrop">
      <div className="dialog" style={{ maxWidth: 520 }}>
        <h2>AI エージェントに権限を付与</h2>
        <p className="dialog-subtitle">
          Claude などの AI に PC 制御を許可する時限式トークンを発行
        </p>

        <div className="field">
          <label>用途ラベル</label>
          <input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="claude-code-session"
            maxLength={64}
          />
        </div>

        <div className="field">
          <label>有効期限 (最大 15 分)</label>
          <select
            value={ttlMins}
            onChange={(e) => setTtlMins(Number(e.target.value))}
          >
            <option value={5}>5分</option>
            <option value={10}>10分</option>
            <option value={15}>15分</option>
          </select>
        </div>

        <div className="field">
          <label>許可する操作</label>
          <div className="caps-list">
            {CAPS.map((cap) => (
              <label key={cap.id} className={`cap-row ${cap.danger ? "danger" : ""}`}>
                <input
                  type="checkbox"
                  checked={selected.has(cap.id)}
                  onChange={() => toggle(cap.id)}
                />
                <span>{cap.label}</span>
                {cap.danger && <span className="danger-badge">確認必須</span>}
              </label>
            ))}
          </div>
        </div>

        <div className="dialog-warning">
          危険操作 (確認必須) は AI が実行を試みるたびに、あなたへ通知が届きます。
          いつでも「信頼済みデバイス」一覧から無効化できます。
        </div>

        {error && <div className="error-msg" style={{ color: "var(--danger, #e05)" }}>{error}</div>}

        <div className="dialog-actions">
          <button className="button-ghost" onClick={onCancel}>キャンセル</button>
          <button
            className="button-primary"
            onClick={issue}
            disabled={selected.size === 0 || !label.trim() || issuing}
          >
            {issuing ? "発行中..." : "発行"}
          </button>
        </div>
      </div>
    </div>
  );
}
