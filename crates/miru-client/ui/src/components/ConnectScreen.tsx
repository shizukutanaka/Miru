import { useEffect, useState } from "react";
import { api, LanPeer, TrustedPeer } from "../lib/tauri";

interface Props {
  onConnect: () => void;
}

export function ConnectScreen({ onConnect }: Props) {
  const [deviceId, setDeviceId] = useState("");
  const [signalUrl, setSignalUrl] = useState(
    localStorage.getItem("miru.signal") || "ws://localhost:21115/ws",
  );
  const [error, setError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [peers, setPeers] = useState<TrustedPeer[]>([]);
  const [lanPeers, setLanPeers] = useState<LanPeer[]>([]);

  useEffect(() => {
    api.listTrustedPeers().then(setPeers);
    const scanLan = () => api.discoverLanPeers().then(setLanPeers).catch(() => {});
    scanLan();
    const interval = setInterval(scanLan, 3000);
    return () => clearInterval(interval);
  }, []);

  const handleConnect = async () => {
    setError(null);
    setConnecting(true);

    const id = deviceId.trim().toUpperCase();
    if (!/^[A-Z0-9]{4}-[A-Z0-9]{4}$/.test(id)) {
      setError("デバイスIDは XXXX-XXXX 形式で入力してください");
      setConnecting(false);
      return;
    }

    if (!/^wss?:\/\/./.test(signalUrl)) {
      setError("シグナルURLは ws:// または wss:// で始まる必要があります");
      setConnecting(false);
      return;
    }

    localStorage.setItem("miru.signal", signalUrl);

    try {
      await api.connect(id, signalUrl);
      onConnect();
    } catch (e) {
      setError(String(e));
      setConnecting(false);
    }
  };

  const handleQuickConnect = async (targetId: string, overrideSignal?: string) => {
    setDeviceId(targetId);
    setError(null);
    setConnecting(true);
    const url = overrideSignal ?? signalUrl;
    if (overrideSignal) setSignalUrl(overrideSignal);
    localStorage.setItem("miru.signal", url);
    try {
      await api.connect(targetId, url);
      onConnect();
    } catch (e) {
      setError(String(e));
      setConnecting(false);
    }
  };

  return (
    <div className="connect-screen">
      <div className="connect-card">
        <h1>接続</h1>
        <p className="subtitle">リモートデバイスのIDを入力</p>

        <div className="field">
          <label htmlFor="connect-device-id">デバイス ID</label>
          <input
            id="connect-device-id"
            className="device-id"
            type="text"
            placeholder="XXXX-XXXX"
            value={deviceId}
            onChange={(e) => setDeviceId(e.target.value)}
            maxLength={9}
            autoFocus
          />
        </div>

        <div className="field">
          <label htmlFor="connect-signal-url">シグナルサーバー</label>
          <input
            id="connect-signal-url"
            type="text"
            value={signalUrl}
            onChange={(e) => setSignalUrl(e.target.value)}
            placeholder="ws://..."
          />
        </div>

        <button
          className="button-primary"
          onClick={handleConnect}
          disabled={connecting || !deviceId}
          aria-busy={connecting}
        >
          {connecting ? "接続中..." : "接続"}
        </button>

        {error && (
          <div className="error-banner" role="alert" aria-live="assertive">
            {error}
          </div>
        )}

        {lanPeers.length > 0 && (
          <div className="peers-list">
            <h3>LAN上のデバイス</h3>
            {lanPeers.map((p) => (
              <div key={p.device_id} className="peer-row">
                <div className="peer-info">
                  <div className="peer-id">{p.friendly_name || p.device_id}</div>
                  <div className="peer-fpr">{p.addresses[0] ?? ""}</div>
                </div>
                <button
                  className="button-ghost"
                  aria-label={`${p.friendly_name || p.device_id} に接続`}
                  onClick={() => {
                    const addr = p.addresses[0];
                    const lanSignal = addr ? `ws://${addr}:${p.port}/ws` : undefined;
                    handleQuickConnect(p.device_id, lanSignal);
                  }}
                  disabled={connecting}
                >
                  接続
                </button>
              </div>
            ))}
          </div>
        )}

        {peers.length > 0 && (
          <div className="peers-list">
            <h3>信頼済みデバイス</h3>
            {peers.map((p) => (
              <div key={p.device_id} className="peer-row">
                <div className="peer-info">
                  <div className="peer-id">{p.friendly_name || p.device_id}</div>
                  <div className="peer-fpr">{p.fingerprint}</div>
                </div>
                <button
                  className="button-ghost"
                  aria-label={`${p.friendly_name || p.device_id} に接続`}
                  onClick={() => handleQuickConnect(p.device_id)}
                  disabled={connecting}
                >
                  接続
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
