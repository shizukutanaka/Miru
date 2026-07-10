import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface AuditSummary {
  total_entries: number;
  chain_intact: boolean;
  last_entry_seq: number;
  last_entry_ts_ms: number;
}

interface AuditEntryView {
  seq: number;
  timestamp_ms: number;
  token_jti: string;
  capability: string;
  action: any;
  confirmed: boolean | null;
  outcome: string;
  hash_short: string;
}

interface Props {
  onClose: () => void;
}

export function AuditLogViewer({ onClose }: Props) {
  const [summary, setSummary] = useState<AuditSummary | null>(null);
  const [entries, setEntries] = useState<AuditEntryView[]>([]);
  const [filter, setFilter] = useState<"all" | "denied" | "ok">("all");
  const [loading, setLoading] = useState(true);

  const refresh = async () => {
    setLoading(true);
    try {
      const [s, e] = await Promise.all([
        invoke<AuditSummary>("audit_summary"),
        invoke<AuditEntryView[]>("audit_entries", { limit: 200 }),
      ]);
      setSummary(s);
      setEntries(e);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, []);

  const filtered = entries.filter((e) => {
    if (filter === "all") return true;
    return e.outcome === filter;
  });

  return (
    <div className="audit-viewer">
      <div className="audit-header">
        <div>
          <h2>Audit Log</h2>
          <p className="audit-subtitle">
            すべての AI エージェント操作の改ざん検知付き記録
          </p>
        </div>
        <button className="button-ghost" onClick={onClose}>閉じる</button>
      </div>

      {summary && (
        <div className="audit-summary">
          <div className="audit-stat">
            <span className="stat-label">エントリ数</span>
            <span className="stat-value">{summary.total_entries.toLocaleString()}</span>
          </div>
          <div className="audit-stat">
            <span className="stat-label">チェーン整合性</span>
            <span
              className={`stat-value ${summary.chain_intact ? "ok" : "error"}`}
              role={summary.chain_intact ? undefined : "alert"}
            >
              {summary.chain_intact ? "OK ✓" : "改ざん検知 ✗"}
            </span>
          </div>
          <div className="audit-stat">
            <span className="stat-label">最終エントリ</span>
            <span className="stat-value">
              {summary.last_entry_ts_ms
                ? new Date(summary.last_entry_ts_ms).toLocaleString()
                : "なし"}
            </span>
          </div>
        </div>
      )}

      <div className="audit-toolbar">
        <div className="filter-group" role="group" aria-label="結果でフィルタ">
          <button
            className={filter === "all" ? "active" : ""}
            aria-pressed={filter === "all"}
            onClick={() => setFilter("all")}
          >
            すべて
          </button>
          <button
            className={filter === "ok" ? "active" : ""}
            aria-pressed={filter === "ok"}
            onClick={() => setFilter("ok")}
          >
            許可
          </button>
          <button
            className={filter === "denied" ? "active" : ""}
            aria-pressed={filter === "denied"}
            onClick={() => setFilter("denied")}
          >
            拒否
          </button>
        </div>
        <button className="button-ghost" onClick={refresh}>
          {loading ? "読込中..." : "更新"}
        </button>
      </div>

      <div className="audit-table">
        {filtered.length === 0 ? (
          <div className="empty-state" role="status">
            まだ記録がありません。AI エージェントが操作するとここに記録されます。
          </div>
        ) : (
          <table>
            <thead>
              <tr>
                <th>#</th>
                <th>時刻</th>
                <th>capability</th>
                <th>操作</th>
                <th>結果</th>
                <th>ハッシュ</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((e) => (
                <tr key={e.seq} className={`row-${e.outcome}`}>
                  <td className="mono">{e.seq}</td>
                  <td className="mono">{formatTime(e.timestamp_ms)}</td>
                  <td className="cap-cell">{e.capability}</td>
                  <td className="action-cell">{summarizeAction(e.action)}</td>
                  <td>
                    <span className={`outcome-badge ${e.outcome}`}>
                      {e.outcome === "ok" ? "許可"
                       : e.outcome === "denied" ? "拒否"
                       : "エラー"}
                    </span>
                    {e.confirmed === true && <span className="confirmed-badge">確認済</span>}
                  </td>
                  <td className="mono hash-cell" title="SHA-256 of this entry">
                    {e.hash_short}…
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

function summarizeAction(action: any): string {
  if (typeof action !== "object" || action === null) return String(action);
  // Common keys → compact form
  if ("x" in action && "y" in action) {
    return `(${(action.x).toFixed(2)}, ${(action.y).toFixed(2)})`;
  }
  if ("text" in action) return `text: "${action.text.slice(0, 30)}…"`;
  if ("len" in action) return `len: ${action.len}`;
  if ("url" in action) return action.url;
  if ("cmd" in action) return `cmd: ${action.cmd}`;
  return JSON.stringify(action).slice(0, 50);
}
