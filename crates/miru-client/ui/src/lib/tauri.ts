import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type SessionStatus = "idle" | "connecting" | "connected" | "disconnected" | "error";

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

export interface SessionEvent {
  kind: SessionStatus;
  message?: string;
  fingerprint?: string;
}

export const api = {
  fingerprint: () => invoke<string>("fingerprint"),

  connect: (deviceId: string, signalUrl: string, pin?: string) =>
    invoke<void>("connect", {
      args: { device_id: deviceId, signal_url: signalUrl, pin },
    }),

  disconnect: () => invoke<void>("disconnect"),

  sendInput: (input: {
    kind: string;
    x?: number; y?: number;
    button?: string;
    key?: number; modifiers?: number;
    dx?: number; dy?: number;
    text?: string;
  }) => invoke<void>("send_input", { args: input }),

  sendClipboard: (text: string) =>
    invoke<void>("send_clipboard", { text }),

  listTrustedPeers: () => invoke<TrustedPeer[]>("list_trusted_peers"),
  revokePeer: (deviceId: string) =>
    invoke<boolean>("revoke_peer", { deviceId }),
  sessionStats: () => invoke<SessionStats>("session_stats"),

  onSessionEvent: (cb: (e: SessionEvent) => void): Promise<UnlistenFn> =>
    listen<SessionEvent>("session-event", (e) => cb(e.payload)),
  onVideoFrame: (cb: (e: VideoFrameEvent) => void): Promise<UnlistenFn> =>
    listen<VideoFrameEvent>("video-frame", (e) => cb(e.payload)),
  onQosUpdate: (cb: (e: { fps: number; bitrate_kbps: number; quality: number }) => void): Promise<UnlistenFn> =>
    listen<{ fps: number; bitrate_kbps: number; quality: number }>("qos-update", (e) => cb(e.payload)),
};
