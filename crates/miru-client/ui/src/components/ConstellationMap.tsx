import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface ConstellationDevice {
  device_id: string;
  name: string;
  form_factor: string;
  status: string;
  last_seen_secs_ago: number;
  addresses: string[];
}

interface Props {
  onConnect: (deviceId: string) => void;
  onClose: () => void;
}

export function ConstellationMap({ onConnect, onClose }: Props) {
  const [devices, setDevices] = useState<ConstellationDevice[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = async () => {
    try {
      const list = await invoke<ConstellationDevice[]>("constellation_devices");
      setDevices(list);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 3000);
    return () => clearInterval(interval);
  }, []);

  // Compute polar positions for the constellation visualization
  const positioned = devices.map((d, i) => {
    const angle = (i / Math.max(devices.length, 1)) * Math.PI * 2 - Math.PI / 2;
    const radius = 140;
    return {
      ...d,
      cx: 200 + Math.cos(angle) * radius,
      cy: 200 + Math.sin(angle) * radius,
    };
  });

  return (
    <div className="constellation-view">
      <div className="audit-header">
        <div>
          <h2>Constellation</h2>
          <p className="audit-subtitle">
            あなたのデバイス群 — クリックで接続、ハンドオフ可能
          </p>
        </div>
        <button className="button-ghost" onClick={onClose}>閉じる</button>
      </div>

      {loading ? (
        <div className="empty-state">読込中...</div>
      ) : devices.length === 0 ? (
        <div className="empty-state">
          まだデバイスが登録されていません。
          <br />
          別のデバイスから constellation key を共有して招待してください。
        </div>
      ) : (
        <>
          <div className="constellation-svg-wrap">
            <svg viewBox="0 0 400 400" width="100%" style={{ maxWidth: 480 }}>
              {/* Center: this device */}
              <g>
                <circle cx={200} cy={200} r={36}
                  fill="rgba(0, 196, 204, 0.15)"
                  stroke="var(--brand)" strokeWidth={2} />
                <circle cx={200} cy={200} r={6} fill="var(--brand)" />
                <text x={200} y={250} textAnchor="middle"
                  fill="var(--fg-secondary)" fontSize={11}>
                  この端末
                </text>
              </g>

              {/* Connection lines */}
              {positioned.map((d) => (
                <line
                  key={`line-${d.device_id}`}
                  x1={200} y1={200}
                  x2={d.cx} y2={d.cy}
                  stroke="var(--border-default)"
                  strokeWidth={1}
                  strokeDasharray={d.status === "active" ? "0" : "4 4"}
                />
              ))}

              {/* Devices */}
              {positioned.map((d) => (
                <g key={d.device_id}
                   onClick={() => onConnect(d.device_id)}
                   style={{ cursor: "pointer" }}>
                  <circle cx={d.cx} cy={d.cy} r={28}
                    fill="var(--bg-elev-2)"
                    stroke={d.status === "active" ? "var(--ok)" : "var(--border-strong)"}
                    strokeWidth={2} />
                  <text x={d.cx} y={d.cy + 4} textAnchor="middle"
                    fontSize={20} fill="var(--fg-primary)"
                    style={{ pointerEvents: "none" }}>
                    {iconFor(d.form_factor)}
                  </text>
                  <text x={d.cx} y={d.cy + 50} textAnchor="middle"
                    fill="var(--fg-secondary)" fontSize={11}>
                    {d.name.slice(0, 14)}
                  </text>
                </g>
              ))}
            </svg>
          </div>

          <div className="device-list">
            {devices.map((d) => (
              <div key={d.device_id} className="device-card">
                <div className="device-icon">{iconFor(d.form_factor)}</div>
                <div className="device-info">
                  <div className="device-name">{d.name}</div>
                  <div className="device-meta">
                    <span className={`status-dot ${d.status}`} />
                    {statusLabel(d.status)}
                    {d.last_seen_secs_ago > 0 && d.last_seen_secs_ago < 86400 * 30 && (
                      <span className="dim">
                        · {formatSeen(d.last_seen_secs_ago)}前
                      </span>
                    )}
                  </div>
                </div>
                <button
                  className="button-ghost"
                  onClick={() => onConnect(d.device_id)}
                  disabled={d.status === "offline"}
                >
                  接続
                </button>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

function iconFor(form: string): string {
  // Plain text glyphs (no emoji per user preference)
  switch (form) {
    case "phone": return "P";
    case "tablet": return "T";
    case "desktop": return "D";
    case "laptop": return "L";
    case "server": return "S";
    case "embedded": return "E";
    default: return "?";
  }
}

function statusLabel(s: string): string {
  switch (s) {
    case "active": return "操作中";
    case "idle": return "アイドル";
    case "sleeping": return "スリープ";
    case "offline": return "オフライン";
    default: return s;
  }
}

function formatSeen(secs: number): string {
  if (secs < 60) return `${secs}秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)}分`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}時間`;
  return `${Math.floor(secs / 86400)}日`;
}
