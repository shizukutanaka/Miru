import { useEffect, useRef, useState } from "react";
import { api, SessionStats, DisplayInfo } from "../lib/tauri";
import { DisplayTabs } from "./DisplayTabs";
import { PairingDialog } from "./PairingDialog";

interface Props {
  onDisconnect: () => void;
}

export function SessionScreen({ onDisconnect }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [stats, setStats] = useState<SessionStats>({
    fps: 0, bitrate_kbps: 0, rtt_ms: 0,
    bytes_recv: 0, frames_decoded: 0, packet_loss_pct: 0,
  });
  const [resolution, setResolution] = useState({ w: 0, h: 0 });
  const [status, setStatus] = useState<
    "connecting" | "connected" | "disconnected" | "reconnecting" | "pairing_required"
  >("connecting");
  const [statusMessage, setStatusMessage] = useState<string | null>(null);
  const [fingerprint, setFingerprint] = useState<string | null>(null);
  const [hostQos, setHostQos] = useState<{ fps: number; bitrate_kbps: number; quality: number } | null>(null);
  const [displays, setDisplays] = useState<DisplayInfo[]>([]);
  const [selectedDisplay, setSelectedDisplay] = useState(0);
  const [fileSending, setFileSending] = useState(false);
  const [recording, setRecording] = useState(false);
  const [elapsedSecs, setElapsedSecs] = useState(0);
  const connectedAtRef = useRef<number | null>(null);
  const [qosMode, setQosMode] = useState<"quality" | "balanced" | "smooth">("balanced");
  const [hostPubAddr, setHostPubAddr] = useState<string | null>(null);

  const connQuality: "good" | "warn" | "poor" =
    stats.rtt_ms > 300 || stats.packet_loss_pct > 10
      ? "poor"
      : stats.rtt_ms > 150 || stats.packet_loss_pct > 3
      ? "warn"
      : "good";

  const lastFrameRef = useRef<number>(Date.now());
  const [stalled, setStalled] = useState(false);

  // Decode pipeline (see ADR 0013): keep only the most recent frame and decode
  // one at a time. createImageBitmap decodes off the main thread (no
  // Image/data-URL main-thread jank); single-flight + latest-wins prevents a
  // decode backlog from building latency when the CPU is constrained.
  const pendingFrameRef = useRef<{ b64: string; w: number; h: number } | null>(null);
  const decodingRef = useRef(false);

  useEffect(() => {
    let unlistenVideo: (() => void) | null = null;
    let unlistenStatus: (() => void) | null = null;
    let unlistenQos: (() => void) | null = null;

    const drainFrames = async () => {
      if (decodingRef.current) return;
      decodingRef.current = true;
      try {
        while (pendingFrameRef.current) {
          const f = pendingFrameRef.current;
          pendingFrameRef.current = null; // claim it; newer frames replace this slot
          const canvas = canvasRef.current;
          if (!canvas) break;
          if (canvas.width !== f.w || canvas.height !== f.h) {
            canvas.width = f.w;
            canvas.height = f.h;
            setResolution({ w: f.w, h: f.h });
          }
          try {
            const bmp = await createImageBitmap(
              new Blob([b64ToBytes(f.b64)], { type: "image/jpeg" }),
            );
            const ctx = canvas.getContext("2d");
            ctx?.drawImage(bmp, 0, 0, canvas.width, canvas.height);
            bmp.close();
          } catch {
            /* corrupt/partial frame: skip and move on to the next */
          }
        }
      } finally {
        decodingRef.current = false;
      }
    };

    api.onVideoFrame((e) => {
      pendingFrameRef.current = { b64: e.jpeg_b64, w: e.width, h: e.height };
      lastFrameRef.current = Date.now();
      setStalled(false);
      void drainFrames();
    }).then((fn) => (unlistenVideo = fn));

    api.onSessionEvent((e) => {
      if (e.kind === "connected") {
        setStatus("connected");
        setStatusMessage(null);
        connectedAtRef.current = Date.now();
        setHostPubAddr(e.host_pub_addr ?? null);
      } else if (e.kind === "reconnecting") {
        setStatus("reconnecting");
        setStatusMessage(e.message ?? null);
      } else if (e.kind === "pairing_required") {
        // First-time connection to this device_id — hold here until the
        // user compares the fingerprint out-of-band and confirms/cancels.
        setStatus("pairing_required");
        setStatusMessage(null);
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
      if (connectedAtRef.current !== null) {
        setElapsedSecs(Math.floor((Date.now() - connectedAtRef.current) / 1000));
        // Detect stall: no frame for > 10 s while connected
        const msSinceFrame = Date.now() - lastFrameRef.current;
        setStalled(msSinceFrame > 10_000);
      }
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

    // Touch: map single touch to mouse, two-finger to scroll
    const normTouch = (t: Touch) => {
      const rect = canvas.getBoundingClientRect();
      return {
        x: Math.max(0, Math.min(1, (t.clientX - rect.left) / rect.width)),
        y: Math.max(0, Math.min(1, (t.clientY - rect.top) / rect.height)),
      };
    };
    let lastTouchPos = { x: 0.5, y: 0.5 };
    let lastPinchDist = 0;

    const onTouchStart = (e: TouchEvent) => {
      e.preventDefault();
      if (e.touches.length === 1) {
        const { x, y } = normTouch(e.touches[0]);
        lastTouchPos = { x, y };
        api.sendInput({ kind: "mouse_move", x, y });
        api.sendInput({ kind: "mouse_down", x, y, button: "left" });
      } else if (e.touches.length === 2) {
        const dx = e.touches[1].clientX - e.touches[0].clientX;
        const dy = e.touches[1].clientY - e.touches[0].clientY;
        lastPinchDist = Math.hypot(dx, dy);
      }
    };
    const onTouchMove = (e: TouchEvent) => {
      e.preventDefault();
      if (e.touches.length === 1) {
        const { x, y } = normTouch(e.touches[0]);
        lastTouchPos = { x, y };
        api.sendInput({ kind: "mouse_move", x, y });
      } else if (e.touches.length === 2) {
        const dx = e.touches[1].clientX - e.touches[0].clientX;
        const dy = e.touches[1].clientY - e.touches[0].clientY;
        const dist = Math.hypot(dx, dy);
        const delta = lastPinchDist > 0 ? (dist - lastPinchDist) / 60 : 0;
        lastPinchDist = dist;
        const cx = (normTouch(e.touches[0]).x + normTouch(e.touches[1]).x) / 2;
        const cy = (normTouch(e.touches[0]).y + normTouch(e.touches[1]).y) / 2;
        api.sendInput({ kind: "scroll", x: cx, y: cy, dx: 0, dy: delta });
      }
    };
    const onTouchEnd = (e: TouchEvent) => {
      e.preventDefault();
      if (e.changedTouches.length > 0) {
        api.sendInput({ kind: "mouse_up", x: lastTouchPos.x, y: lastTouchPos.y, button: "left" });
      }
      lastPinchDist = 0;
    };

    canvas.addEventListener("mousemove", onMove);
    canvas.addEventListener("mousedown", onDown);
    canvas.addEventListener("mouseup", onUp);
    canvas.addEventListener("wheel", onWheel, { passive: false });
    canvas.addEventListener("contextmenu", onContext);
    canvas.addEventListener("touchstart", onTouchStart, { passive: false });
    canvas.addEventListener("touchmove", onTouchMove, { passive: false });
    canvas.addEventListener("touchend", onTouchEnd, { passive: false });

    return () => {
      canvas.removeEventListener("mousemove", onMove);
      canvas.removeEventListener("mousedown", onDown);
      canvas.removeEventListener("mouseup", onUp);
      canvas.removeEventListener("wheel", onWheel);
      canvas.removeEventListener("contextmenu", onContext);
      canvas.removeEventListener("touchstart", onTouchStart);
      canvas.removeEventListener("touchmove", onTouchMove);
      canvas.removeEventListener("touchend", onTouchEnd);
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
      // Cap at 1 MiB — the host enforces 16 MiB but sending huge payloads
      // over the relay wastes bandwidth and could stall the session loop.
      if (text.length > 1024 * 1024) return;
      await api.sendClipboard(text);
    } catch {}
  };

  const handleRequestClipboard = async () => {
    try { await api.requestClipboard(); } catch {}
  };

  const cycleQosMode = async () => {
    const next = qosMode === "balanced" ? "quality" : qosMode === "quality" ? "smooth" : "balanced";
    setQosMode(next);
    try { await api.sendQosHint(next); } catch {}
  };

  const handleToggleRecording = async () => {
    if (recording) {
      try { await api.stopRecording(); } catch {}
      setRecording(false);
    } else {
      try { await api.startRecording(); setRecording(true); } catch {}
    }
  };

  const handleFileSend = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    // Reset input so same file can be re-sent
    e.target.value = "";
    if (file.size > 100 * 1024 * 1024) {
      alert("ファイルサイズが 100 MB を超えています");
      return;
    }
    setFileSending(true);
    try {
      const buf = await file.arrayBuffer();
      // Convert to base64
      const bytes = new Uint8Array(buf);
      let binary = "";
      for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
      const b64 = btoa(binary);
      await api.sendFile(file.name, b64);
    } catch (err) {
      console.error("File send failed:", err);
    } finally {
      setFileSending(false);
    }
  };

  return (
    <div className="session">
      {status === "pairing_required" && (
        <PairingDialog
          fingerprint={fingerprint ?? ""}
          requirePin={false}
          onConfirm={() => {
            void api.confirmPairing(true);
          }}
          onCancel={() => {
            void api.confirmPairing(false);
            onDisconnect();
          }}
        />
      )}

      <div className="video-stage" ref={stageRef}>
        <canvas ref={canvasRef} tabIndex={0} aria-label="リモートホストの画面" role="img" />

        <div className="session-overlay" role="status" aria-live="polite" aria-label="接続統計">
          <div className="stat-row"><span className="label">FPS</span><span className="value">{stats.fps.toFixed(1)}</span></div>
          <div className="stat-row"><span className="label">RTT</span><span className="value">{stats.rtt_ms} ms</span></div>
          <div className="stat-row"><span className="label">BW</span><span className="value">{stats.bitrate_kbps} kbps</span></div>
          {elapsedSecs > 0 && (
            <div className="stat-row"><span className="label">T</span><span className="value">{fmtDuration(elapsedSecs)}</span></div>
          )}
          <div className="stat-row"><span className="label">RX</span><span className="value">{(stats.bytes_recv / 1024 / 1024).toFixed(1)} MB</span></div>
          {stats.packet_loss_pct > 0.5 && (
            <div className="stat-row"><span className="label" style={{ color: "var(--warn, #f90)" }}>PKT</span><span className="value">{stats.packet_loss_pct.toFixed(1)}%</span></div>
          )}
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
          <div className="stat-row" title={hostPubAddr ? `ホスト公開アドレス: ${hostPubAddr}` : "中継経由で接続"}>
            <span className="label">経路</span>
            <span className="value">{hostPubAddr ? `リレー (直接: ${hostPubAddr})` : "リレー"}</span>
          </div>
        </div>
      </div>

      <DisplayTabs displays={displays} selected={selectedDisplay} onSelect={handleSelectDisplay} />

      {status === "connected" && stalled && (
        <div className="quality-banner quality-poor" role="alert">
          映像が停止しています — ホストが応答していない可能性があります
        </div>
      )}
      {status === "connected" && !stalled && connQuality !== "good" && (
        <div className={`quality-banner quality-${connQuality}`} role="status" aria-live="polite">
          {connQuality === "poor"
            ? `接続が不安定 — RTT ${stats.rtt_ms}ms / 損失 ${stats.packet_loss_pct.toFixed(1)}%`
            : `接続状態が低下 — RTT ${stats.rtt_ms}ms`}
        </div>
      )}

      <input
        ref={fileInputRef}
        type="file"
        aria-label="ホストへ送信するファイルを選択"
        style={{ display: "none" }}
        onChange={handleFileSend}
      />

      <div className="toolbar">
        <span className="status-pill" role="status" aria-live="polite">
          <span className={`status-dot ${
            status === "connected" ? "ok"
            : status === "reconnecting" || status === "pairing_required" ? "warn"
            : status === "connecting" ? "info"
            : "error"
          }`} />
          {status === "connected" ? "接続中"
            : status === "pairing_required" ? "指紋の確認待ち"
            : status === "reconnecting" ? (statusMessage ?? "再接続中...")
            : status === "connecting" ? "接続しています"
            : "切断"}
        </span>
        <div className="toolbar-spacer" />
        <button
          onClick={handleToggleRecording}
          className={recording ? "danger" : ""}
          disabled={status !== "connected"}
          title={recording ? "録画を停止" : "録画を開始"}
        >
          {recording ? "録画停止" : "録画"}
        </button>
        <button onClick={handleClipboardSync} title="ローカルのクリップボードをホストへ送信">
          クリップボード送信
        </button>
        <button onClick={handleRequestClipboard} title="ホストのクリップボードを受信" disabled={status !== "connected"}>
          クリップボード受信
        </button>
        <button
          onClick={cycleQosMode}
          title="画質モード: 画質重視 / バランス / 滑らか"
          disabled={status !== "connected"}
        >
          {qosMode === "quality" ? "画質重視" : qosMode === "smooth" ? "滑らか" : "バランス"}
        </button>
        <button
          onClick={() => fileInputRef.current?.click()}
          disabled={fileSending || status !== "connected"}
        >
          {fileSending ? "送信中..." : "ファイル送信"}
        </button>
        <button onClick={handleFullscreen}>フルスクリーン</button>
        <button className="danger" onClick={handleDisconnect}>切断</button>
      </div>
    </div>
  );
}

/** Decode base64 → ArrayBuffer without an intermediate data-URL string. */
function b64ToBytes(b64: string): ArrayBuffer {
  const bin = atob(b64);
  const buf = new ArrayBuffer(bin.length);
  const view = new Uint8Array(buf);
  for (let i = 0; i < bin.length; i++) view[i] = bin.charCodeAt(i);
  return buf;
}

function fmtDuration(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}
