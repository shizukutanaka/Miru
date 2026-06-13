import { useEffect, useRef, useState } from "react";
import { api, SessionStats, DisplayInfo } from "../lib/tauri";
import { DisplayTabs } from "./DisplayTabs";

interface Props {
  onDisconnect: () => void;
}

export function SessionScreen({ onDisconnect }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [stats, setStats] = useState<SessionStats>({
    fps: 0, bitrate_kbps: 0, rtt_ms: 0,
    bytes_recv: 0, frames_decoded: 0, packet_loss_pct: 0,
  });
  const [resolution, setResolution] = useState({ w: 0, h: 0 });
  const [status, setStatus] = useState<"connecting" | "connected" | "disconnected" | "reconnecting">("connecting");
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [fingerprint, setFingerprint] = useState<string | null>(null);
  const [hostQos, setHostQos] = useState<{ fps: number; bitrate_kbps: number; quality: number } | null>(null);
  const [displays, setDisplays] = useState<DisplayInfo[]>([]);
  const [selectedDisplay, setSelectedDisplay] = useState(0);

  // Pre-allocate Image to reuse across frames (avoids GC pressure)
  const imgRef = useRef<HTMLImageElement | null>(null);
  if (!imgRef.current) imgRef.current = new Image();

  useEffect(() => {
    let unlistenVideo: (() => void) | null = null;
    let unlistenStatus: (() => void) | null = null;
    let unlistenQos: (() => void) | null = null;

    api.onVideoFrame((e) => {
      const canvas = canvasRef.current;
      if (!canvas) return;

      if (canvas.width !== e.width || canvas.height !== e.height) {
        canvas.width = e.width;
        canvas.height = e.height;
        setResolution({ w: e.width, h: e.height });
      }

      const img = imgRef.current!;
      img.onload = () => {
        const ctx = canvas.getContext("2d");
        ctx?.drawImage(img, 0, 0, canvas.width, canvas.height);
      };
      img.src = `data:image/jpeg;base64,${e.jpeg_b64}`;
    }).then((fn) => (unlistenVideo = fn));

    api.onSessionEvent((e) => {
      if (e.kind === "connected") {
        setStatus("connected");
        setStatusMessage(null);
      } else if (e.kind === "reconnecting") {
        setStatus("reconnecting");
        setStatusMessage(e.message ?? null);
      } else if (e.kind === "disconnected" || e.kind === "error") {
        setStatus("disconnected");
        setStatusMessage(e.message ?? null);
        setTimeout(onDisconnect, 1200);
      } else {
        setStatus("connecting");
      }
      if (e.fingerprint) setFingerprint(e.fingerprint);
    }).then((fn) => (unlistenStatus = fn));

    api.onQosUpdate((q) => setHostQos(q)).then((fn) => (unlistenQos = fn));

    let unlistenDisplays: (() => void) | null = null;
    api.onDisplayList((d) => setDisplays(d)).then((fn) => (unlistenDisplays = fn));

    let unlistenClipboard: (() => void) | null = null;
    api.onClipboardSync((text) => {
      navigator.clipboard.writeText(text).catch(() => {});
    }).then((fn) => (unlistenClipboard = fn));

    const statsInterval = setInterval(async () => {
      try { setStats(await api.sessionStats()); } catch {}
    }, 500);

    return () => {
      unlistenVideo?.();
      unlistenStatus?.();
      unlistenQos?.();
      unlistenDisplays?.();
      unlistenClipboard?.();
      clearInterval(statsInterval);
    };
  }, [onDisconnect]);

  // Mouse: normalize coordinates and forward
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const norm = (e: MouseEvent) => {
      const rect = canvas.getBoundingClientRect();
      return {
        x: Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)),
        y: Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height)),
      };
    };

    const buttonName = (b: number) =>
      b === 0 ? "left" : b === 1 ? "middle" : b === 2 ? "right" : "left";

    const onMove = (e: MouseEvent) => {
      const { x, y } = norm(e);
      api.sendInput({ kind: "mouse_move", x, y });
    };
    const onDown = (e: MouseEvent) => {
      const { x, y } = norm(e);
      api.sendInput({ kind: "mouse_down", x, y, button: buttonName(e.button) });
    };
    const onUp = (e: MouseEvent) => {
      const { x, y } = norm(e);
      api.sendInput({ kind: "mouse_up", x, y, button: buttonName(e.button) });
    };
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { x, y } = norm(e);
      api.sendInput({ kind: "scroll", x, y, dx: -e.deltaX / 100, dy: -e.deltaY / 100 });
    };
    const onContext = (e: Event) => e.preventDefault();

    canvas.addEventListener("mousemove", onMove);
    canvas.addEventListener("mousedown", onDown);
    canvas.addEventListener("mouseup", onUp);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    canvas.addEventListener("contextmenu", onContext);

    return () => {
      canvas.removeEventListener("mousemove", onMove);
      canvas.removeEventListener("mousedown", onDown);
      canvas.removeEventListener("mouseup", onUp);
      canvas.removeEventListener("wheel", onWheel);
      canvas.removeEventListener("contextmenu", onContext);
    };
  }, []);

  // Keyboard
  useEffect(() => {
    const onKey = (down: boolean) => (e: KeyboardEvent) => {
      // Don't intercept window-management keys
      if ((e.ctrlKey || e.metaKey) && ["q", "w", "h", "m"].includes(e.key.toLowerCase())) return;
      e.preventDefault();
      const mods =
        (e.shiftKey ? 0x01 : 0) |
        (e.ctrlKey ? 0x02 : 0) |
        (e.altKey ? 0x04 : 0) |
        (e.metaKey ? 0x08 : 0);
      api.sendInput({
        kind: down ? "key_down" : "key_up",
        key: e.keyCode,
        modifiers: mods,
      });
    };
    const dn = onKey(true);
    const up = onKey(false);
    window.addEventListener("keydown", dn);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", dn);
      window.removeEventListener("keyup", up);
    };
  }, []);

  const handleDisconnect = async () => {
    await api.disconnect();
    onDisconnect();
  };

  const handleSelectDisplay = async (index: number) => {
    setSelectedDisplay(index);
    try { await api.selectDisplay(index); } catch {}
  };

  const handleFullscreen = () => {
    const stage = stageRef.current;
    if (!stage) return;
    if (document.fullscreenElement) document.exitFullscreen();
    else stage.requestFullscreen?.();
  };

  const handleClipboardSync = async () => {
    try {
      const text = await navigator.clipboard.readText();
      await api.sendClipboard(text);
    } catch {}
  };

  return (
    <div className="session">
      <div className="video-stage" ref={stageRef}>
        <canvas ref={canvasRef} tabIndex={0} />

        <div className="session-overlay">
          <div className="stat-row"><span className="label">FPS</span><span className="value">{stats.fps.toFixed(1)}</span></div>
          <div className="stat-row"><span className="label">RTT</span><span className="value">{stats.rtt_ms} ms</span></div>
          <div className="stat-row"><span className="label">RX</span><span className="value">{(stats.bytes_recv / 1024 / 1024).toFixed(1)} MB</span></div>
          {resolution.w > 0 && (
            <div className="stat-row"><span className="label">RES</span><span className="value">{resolution.w}×{resolution.h}</span></div>
          )}
          {hostQos && (
            <div className="stat-row">
              <span className="label">QoS</span>
              <span className="value">{hostQos.fps}fps · {hostQos.bitrate_kbps}kbps · q{hostQos.quality}</span>
            </div>
          )}
          {fingerprint && (
            <div className="stat-row fingerprint-row" title="ホストの指紋 — 帯域外で確認してください">
              <span className="label">FPR</span>
              <span className="value fingerprint">{fingerprint}</span>
            </div>
          )}
        </div>
      </div>

      <DisplayTabs displays={displays} selected={selectedDisplay} onSelect={handleSelectDisplay} />

      <div className="toolbar">
        <span className="status-pill">
          <span className={`status-dot ${
            status === "connected" ? "ok"
            : status === "reconnecting" ? "warn"
            : status === "connecting" ? "info"
            : "error"
          }`} />
          {status === "connected" ? "接続中"
            : status === "reconnecting" ? (statusMessage ?? "再接続中...")
            : status === "connecting" ? "接続しています"
            : "切断"}
        </span>
        <div className="toolbar-spacer" />
        <button onClick={handleClipboardSync}>クリップボード送信</button>
        <button onClick={handleFullscreen}>フルスクリーン</button>
        <button className="danger" onClick={handleDisconnect}>切断</button>
      </div>
    </div>
  );
}
