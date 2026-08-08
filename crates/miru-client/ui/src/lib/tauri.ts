import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type SessionStatus =
  | "idle"
  | "connecting"
  | "connected"
  | "disconnected"
  | "error"
  | "reconnecting"
  | "pairing_required";

export interface SessionStats {
  fps: number;
  bitrate_kbps: number;
  rtt_ms: number;
  bytes_recv: number;
  frames_decoded: number;
  packet_loss_pct: number;
}

export interface TrustedPeer {
  device_id: string;
  pubkey_b64: string;
  fingerprint: string;
  permission: "view_only" | "control" | "full";
  first_seen: number;
  last_seen: number;
  friendly_name?: string;
}

export interface VideoFrameEvent {
  width: number;
  height: number;
  keyframe: boolean;
  jpeg_b64: string;
}

export interface VideoPacketEvent {
  /** "vp9" | "vp8" — mapped to a WebCodecs codec string by the renderer. */
  codec: string;
  keyframe: boolean;
  timestamp_ms: number;
  data_b64: string;
}

export interface SessionEvent {
  kind: SessionStatus;
  message?: string;
  fingerprint?: string;
  host_pub_addr?: string;
  /** Host can actually send system audio (present on the "connected" event). */
  audio_available?: boolean;
}

export const api = {
  fingerprint: () => invoke<string>("fingerprint"),

  connect: (deviceId: string, signalUrl: string) =>
    invoke<void>("connect", {
      args: { device_id: deviceId, signal_url: signalUrl },
    }),

  disconnect: () => invoke<void>("disconnect"),

  /** Resolve a pending first-connection TOFU fingerprint confirmation. */
  confirmPairing: (accept: boolean) =>
    invoke<void>("confirm_pairing", { accept }),

  /**
   * Choose the video decode path. `webcodecs = true` forwards raw VP9/VP8
   * packets for the WebView's VideoDecoder; `false` decodes in Rust and emits
   * JPEG frames (needed for recording).
   */
  setDecodeMode: (webcodecs: boolean) =>
    invoke<void>("set_decode_mode", { webcodecs }),

  /** Mute/unmute inbound host audio (drops frames before decode). */
  setAudioMuted: (muted: boolean) =>
    invoke<void>("set_audio_muted", { muted }),

  sendInput: (input: {
    kind: string;
    x?: number; y?: number;
    button?: string;
    key?: number; modifiers?: number;
    /** W3C physical-key id ("KeyA") — layout-independent; hosts prefer this. */
    code?: string;
    dx?: number; dy?: number;
    text?: string;
  }) => invoke<void>("send_input", { args: input }),

  sendClipboard: (text: string) =>
    invoke<void>("send_clipboard", { text }),

  requestClipboard: () => invoke<void>("request_clipboard"),

  sendQosHint: (mode: "quality" | "balanced" | "smooth", maxFps = 0, minQuality = 0) =>
    invoke<void>("send_qos_hint", { mode, maxFps, minQuality }),

  sendFile: (name: string, dataB64: string) =>
    invoke<void>("send_file", { name, dataB64 }),

  listTrustedPeers: () => invoke<TrustedPeer[]>("list_trusted_peers"),
  revokePeer: (deviceId: string) =>
    invoke<boolean>("revoke_peer", { deviceId }),
  sessionStats: () => invoke<SessionStats>("session_stats"),

  issueAgentToken: (label: string, ttlMins: number, capabilities: string[]) =>
    invoke<{ token: string; fingerprint: string; expires_at: number }>(
      "issue_agent_token",
      { args: { label, ttl_mins: ttlMins, capabilities } }
    ),

  onSessionEvent: (cb: (e: SessionEvent) => void): Promise<UnlistenFn> =>
    listen<SessionEvent>("session-event", (e) => cb(e.payload)),
  onVideoFrame: (cb: (e: VideoFrameEvent) => void): Promise<UnlistenFn> =>
    listen<VideoFrameEvent>("video-frame", (e) => cb(e.payload)),
  onVideoPacket: (cb: (e: VideoPacketEvent) => void): Promise<UnlistenFn> =>
    listen<VideoPacketEvent>("video-packet", (e) => cb(e.payload)),
  onQosUpdate: (cb: (e: { fps: number; bitrate_kbps: number; quality: number }) => void): Promise<UnlistenFn> =>
    listen<{ fps: number; bitrate_kbps: number; quality: number }>("qos-update", (e) => cb(e.payload)),
  onDisplayList: (cb: (displays: DisplayInfo[]) => void): Promise<UnlistenFn> =>
    listen<{ displays: DisplayInfo[] }>("display-list", (e) => cb(e.payload.displays)),
  onClipboardSync: (cb: (text: string) => void): Promise<UnlistenFn> =>
    listen<string>("clipboard-sync", (e) => cb(e.payload)),
  selectDisplay: (index: number) =>
    invoke<void>("select_display", { index }),
  discoverLanPeers: () => invoke<LanPeer[]>("discover_lan_peers"),
  startRecording: () => invoke<string>("start_recording"),
  stopRecording: () => invoke<RecordingSummary>("stop_recording"),
  listRecordings: () => invoke<RecordingSummary[]>("list_recordings"),
  getRecordingFrame: (recordingPath: string, frameIdx: number) =>
    invoke<string>("get_recording_frame", { recordingPath, frameIdx }),
};

export interface DisplayInfo {
  index: number;
  width: number;
  height: number;
  refresh_hz: number;
  name: string;
  primary: boolean;
}

export interface LanPeer {
  device_id: string;
  friendly_name: string;
  addresses: string[];
  port: number;
  form_factor: string;
}

export interface RecordingSummary {
  path: string;
  session_id: string;
  start_ts_ms: number;
  duration_ms: number;
  frame_count: number;
  size_bytes: number;
}
