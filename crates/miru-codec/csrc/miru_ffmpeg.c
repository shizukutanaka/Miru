// Minimal C surface over libavcodec, for hardware (and software) video encode.
//
// WHY A C SHIM RATHER THAN A RUST BINDING CRATE
//
// ffmpeg-next was the planned route, but it is a crates.io dependency and it is
// not needed: libavcodec is a C library with a stable public API, and the only
// hard part of binding it from Rust is that AVCodecContext/AVFrame/AVPacket are
// direct-access structs whose layout differs between ffmpeg majors. Declaring
// those layouts in Rust would bake an ABI assumption into the build.
//
// This shim keeps every ffmpeg struct on the C side, compiled against the real
// headers, so layout is correct by construction and Rust sees only an opaque
// pointer plus scalars. The result needs no Rust dependency at all.
//
// Receive is split in two so a packet is never silently truncated: _begin
// reports the size of the packet that is ready, _copy hands it over and frees
// it. The packet stays owned by this struct in between.

#include <libavcodec/avcodec.h>
#include <libavutil/opt.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    AVCodecContext *ctx;
    AVFrame *frame;
    AVPacket *pkt;
    int have_pkt;
    int64_t pts;
} MiruEnc;

void miru_enc_close(MiruEnc *e);

/// Is this encoder compiled into the linked libavcodec? Says nothing about
/// whether the silicon it needs is present — opening it is the real test.
int miru_enc_available(const char *name) {
    return avcodec_find_encoder_by_name(name) != NULL;
}

/// Version of the libavcodec actually linked, for diagnostics.
unsigned miru_enc_version(void) { return avcodec_version(); }

MiruEnc *miru_enc_open(const char *name, int w, int h, int fps,
                       int64_t bitrate_bps, int gop) {
    const AVCodec *codec = avcodec_find_encoder_by_name(name);
    if (!codec || w <= 0 || h <= 0 || fps <= 0) return NULL;

    MiruEnc *e = calloc(1, sizeof(MiruEnc));
    if (!e) return NULL;

    e->ctx = avcodec_alloc_context3(codec);
    if (!e->ctx) { free(e); return NULL; }

    e->ctx->width = w;
    e->ctx->height = h;
    e->ctx->time_base = (AVRational){1, fps};
    e->ctx->framerate = (AVRational){fps, 1};
    e->ctx->pix_fmt = AV_PIX_FMT_YUV420P;
    e->ctx->bit_rate = bitrate_bps;
    e->ctx->rc_max_rate = bitrate_bps * 3 / 2;
    e->ctx->gop_size = gop;
    // Remote desktop is interactive: B-frames reorder output and add a frame of
    // latency for no benefit here.
    e->ctx->max_b_frames = 0;

    // Low-latency knobs. Each encoder family names them differently and
    // av_opt_set simply fails on the ones that do not apply, which is fine —
    // we are opportunistically setting whichever the chosen encoder honours.
    av_opt_set(e->ctx->priv_data, "tune", "zerolatency", 0);   // x264/x265
    av_opt_set(e->ctx->priv_data, "preset", "p1", 0);          // nvenc (fastest)
    av_opt_set(e->ctx->priv_data, "delay", "0", 0);            // nvenc
    av_opt_set(e->ctx->priv_data, "async_depth", "1", 0);      // qsv/vaapi
    av_opt_set(e->ctx->priv_data, "usage", "ultralowlatency", 0); // amf

    if (avcodec_open2(e->ctx, codec, NULL) < 0) { miru_enc_close(e); return NULL; }

    e->frame = av_frame_alloc();
    e->pkt = av_packet_alloc();
    if (!e->frame || !e->pkt) { miru_enc_close(e); return NULL; }
    e->frame->format = AV_PIX_FMT_YUV420P;
    e->frame->width = w;
    e->frame->height = h;
    if (av_frame_get_buffer(e->frame, 0) < 0) { miru_enc_close(e); return NULL; }
    return e;
}

/// Submit one I420 frame: Y plane then U then V, tightly packed, no padding.
/// Copies plane by plane because the encoder's own buffer is stride-aligned.
int miru_enc_send(MiruEnc *e, const uint8_t *i420, size_t len, int keyframe) {
    if (!e || !i420) return -1;
    int w = e->ctx->width, h = e->ctx->height;
    size_t need = (size_t)w * h + 2 * (size_t)(w / 2) * (h / 2);
    if (len < need) return -1;
    if (av_frame_make_writable(e->frame) < 0) return -1;

    const uint8_t *src[3] = {
        i420,
        i420 + (size_t)w * h,
        i420 + (size_t)w * h + (size_t)(w / 2) * (h / 2),
    };
    int src_stride[3] = { w, w / 2, w / 2 };
    for (int p = 0; p < 3; p++) {
        int rows = p ? h / 2 : h;
        for (int y = 0; y < rows; y++) {
            memcpy(e->frame->data[p] + (size_t)y * e->frame->linesize[p],
                   src[p] + (size_t)y * src_stride[p], (size_t)src_stride[p]);
        }
    }
    e->frame->pts = e->pts++;
    // Ask for an IDR; encoders that cannot honour it just ignore the hint.
    e->frame->pict_type = keyframe ? AV_PICTURE_TYPE_I : AV_PICTURE_TYPE_NONE;
    return avcodec_send_frame(e->ctx, e->frame);
}

/// 1 = a packet is ready (its size and keyframe flag are written out),
/// 0 = encoder needs more input, <0 = error.
int miru_enc_recv_begin(MiruEnc *e, int *size, int *is_key) {
    if (!e || e->have_pkt) return -1;
    int r = avcodec_receive_packet(e->ctx, e->pkt);
    if (r == AVERROR(EAGAIN) || r == AVERROR_EOF) return 0;
    if (r < 0) return r;
    e->have_pkt = 1;
    *size = e->pkt->size;
    *is_key = (e->pkt->flags & AV_PKT_FLAG_KEY) ? 1 : 0;
    return 1;
}

/// Copy the pending packet out and release it. `cap` must be >= the size
/// reported by _begin.
int miru_enc_recv_copy(MiruEnc *e, uint8_t *out, int cap) {
    if (!e || !e->have_pkt || !out) return -1;
    int n = e->pkt->size;
    if (cap < n) return -1;
    memcpy(out, e->pkt->data, (size_t)n);
    av_packet_unref(e->pkt);
    e->have_pkt = 0;
    return n;
}

void miru_enc_set_bitrate(MiruEnc *e, int64_t bitrate_bps) {
    if (!e || bitrate_bps <= 0) return;
    // Takes effect on encoders that read bit_rate per-frame (nvenc, vaapi).
    e->ctx->bit_rate = bitrate_bps;
    e->ctx->rc_max_rate = bitrate_bps * 3 / 2;
}

void miru_enc_close(MiruEnc *e) {
    if (!e) return;
    if (e->pkt) { if (e->have_pkt) av_packet_unref(e->pkt); av_packet_free(&e->pkt); }
    if (e->frame) av_frame_free(&e->frame);
    if (e->ctx) avcodec_free_context(&e->ctx);
    free(e);
}
