//! Wire protocol message types (MessagePack over QUIC/WebSocket).
//!
//! Design: flat enum, no inheritance. Each variant is self-contained.
//! Framing: [4-byte LE length][msgpack payload]

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Codec capability ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    Av1,
    H265,
    H264,
    Vp9,
    Vp8,
    /// Baseline JPEG fallback — always available, no native codec dep.
    /// Higher bandwidth than VP9 but zero build-time dependencies.
    Jpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Opus,
    Pcm,
}

// ─── Session roles ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Hosting the screen / accepting input.
    Host,
    /// Watching the screen / sending input — typically a human.
    Viewer,
    /// AI agent viewer — capability-scoped, audit-logged. Not a human.
    /// Connections of this role go through `miru-agent` token verification
    /// and every input goes through an authorize() gate.
    AiAgent,
}

// ─── Main message enum ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    // Handshake
    Hello(Hello),
    HelloAck(HelloAck),

    // Signaling (rendezvous server)
    Register(Register),
    RegisterAck(RegisterAck),
    Connect(ConnectRequest),
    ConnectAck(ConnectAck),
    Relay(RelayOffer),

    // Session negotiation
    Offer(SessionOffer),
    Answer(SessionAnswer),

    // Media
    VideoFrame(VideoFrame),
    AudioFrame(AudioFrame),

    // Control
    InputEvent(InputEvent),
    ClipboardSync(ClipboardSync),
    FileTransfer(FileTransfer),
    Ping(Ping),
    Pong(Pong),

    // Display management
    DisplayList(DisplayList),
    SelectDisplay(SelectDisplay),
    ResolutionChange(ResolutionChange),

    // Quality control
    QosUpdate(QosUpdate),
    QosHint(QosHint),
    KeyFrame,

    // Clipboard pull
    RequestClipboard,

    // Session lifecycle
    Close(CloseReason),
    Error(ErrorMsg),
}

// ─── Handshake ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub role: Role,
    /// X25519 ephemeral public key (32 bytes, base64)
    pub pubkey: String,
    pub features: Features,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Features {
    pub codecs: Vec<VideoCodec>,
    pub audio_codecs: Vec<AudioCodec>,
    pub hw_encode: bool,
    pub hw_decode: bool,
    pub clipboard: bool,
    pub file_transfer: bool,
    pub audio: bool,
    pub multi_monitor: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    pub session_id: Uuid,
    pub pubkey: String,
    /// AEAD-encrypted symmetric key material
    pub encrypted_key: Vec<u8>,
    pub selected_codec: VideoCodec,
    pub selected_audio: AudioCodec,
}

// ─── Signaling ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Register {
    pub device_id: String,
    pub pubkey: String,
    /// Public (STUN-discovered) address of this device, for direct-path negotiation.
    /// None when behind symmetric NAT or when STUN failed.
    pub pub_addr: Option<String>,
    pub pub_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAck {
    pub device_id: String,
    pub relay_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectRequest {
    pub target_id: String,
    pub viewer_addr: String,
    pub viewer_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectAck {
    pub target_id: String,
    pub host_addr: Option<String>,
    pub host_port: Option<u16>,
    pub relay: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayOffer {
    pub relay_addr: String,
    pub relay_port: u16,
    pub token: String,
}

// ─── Session ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOffer {
    pub codec: VideoCodec,
    pub audio_codec: AudioCodec,
    pub display: DisplayInfo,
    pub fps: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAnswer {
    pub accepted: bool,
    pub fps: u8,
    pub bitrate_kbps: u32,
}

// ─── Color space (HDR support, Sunshine-style) ────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorPrimaries {
    /// Standard sRGB / BT.709 (default for HD/SDR).
    #[default]
    Bt709,
    /// BT.2020 (UHD / HDR).
    Bt2020,
    /// DCI-P3 (cinema, some HDR displays).
    DciP3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransferFunction {
    /// SDR — standard gamma.
    #[default]
    Bt709,
    /// HDR10 — perceptual quantizer (PQ, SMPTE ST.2084).
    Pq,
    /// HDR HLG (BBC/NHK Hybrid Log-Gamma).
    Hlg,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorRange {
    /// 16-235 for Y, 16-240 for UV (broadcast/MPEG).
    #[default]
    Limited,
    /// 0-255 (PC / web).
    Full,
}

/// HDR static metadata (SMPTE ST.2086 Mastering Display).
/// Sent only when codec encodes HDR.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HdrMetadata {
    /// Mastering display max luminance (cd/m²).
    pub max_luminance: u32,
    /// Mastering display min luminance (1/10000 cd/m²).
    pub min_luminance: u32,
    /// MaxCLL — Max Content Light Level (cd/m²).
    pub max_content_light: u32,
    /// MaxFALL — Max Frame-Average Light Level (cd/m²).
    pub max_frame_avg_light: u32,
}

// ─── Media ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoFrame {
    pub seq: u64,
    pub display_idx: u8,
    pub keyframe: bool,
    pub codec: VideoCodec,
    /// Encoded payload (may be encrypted at transport layer)
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp_ms: u64,
    /// Color space (default BT.709 SDR for backward compatibility).
    #[serde(default)]
    pub color_primaries: ColorPrimaries,
    #[serde(default)]
    pub transfer: TransferFunction,
    #[serde(default)]
    pub color_range: ColorRange,
    /// Present only on HDR keyframes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr_metadata: Option<HdrMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub seq: u64,
    pub codec: AudioCodec,
    pub data: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u8,
    pub timestamp_ms: u64,
}

// ─── Input ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputEvent {
    pub kind: InputKind,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum InputKind {
    MouseMove { x: f32, y: f32, display: u8 },
    MouseDown { button: MouseButton, x: f32, y: f32 },
    MouseUp { button: MouseButton, x: f32, y: f32 },
    Scroll { dx: f32, dy: f32, x: f32, y: f32 },
    KeyDown { key: u32, modifiers: u8 },
    KeyUp { key: u32, modifiers: u8 },
    Text { text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

// ─── Clipboard ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardSync {
    pub format: ClipboardFormat,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardFormat {
    Text,
    Html,
    Image,
}

// ─── File transfer ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileTransfer {
    Start {
        id: Uuid,
        name: String,
        size: u64,
        hash: String,
    },
    Chunk {
        id: Uuid,
        offset: u64,
        data: Vec<u8>,
    },
    Done {
        id: Uuid,
    },
    Abort {
        id: Uuid,
        reason: String,
    },
}

// ─── Display ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub index: u8,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u8,
    pub name: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayList {
    pub displays: Vec<DisplayInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectDisplay {
    pub index: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolutionChange {
    pub width: u32,
    pub height: u32,
}

// ─── QoS ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QosUpdate {
    pub fps: u8,
    pub bitrate_kbps: u32,
    pub quality: u8, // 0–100
}

// ─── QoS hint (viewer → host) ─────────────────────────────────────────────────

/// Viewer preference hint that the host QoS controller takes into account.
/// Sent when the viewer changes mode via UI; does not override network-driven adjustments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QosHint {
    /// "quality" | "balanced" | "smooth"
    pub mode: String,
    /// Optional hard cap on FPS (0 = no cap).
    pub max_fps: u8,
    /// Optional quality floor 0–100 (0 = no floor).
    pub min_quality: u8,
}

// ─── Misc ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ping {
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pong {
    pub ts: u64,
    pub server_ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseReason {
    pub code: u16,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: u16,
    pub message: String,
}
