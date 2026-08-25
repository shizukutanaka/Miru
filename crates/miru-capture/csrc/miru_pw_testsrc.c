// Test-only PipeWire video producer.
//
// NOT part of the library build — build.rs compiles only miru_portal.c. This
// exists so the stream consumer can be tested against a real PipeWire daemon
// without needing a compositor, a portal, or an external video source: the
// test brings up its own producer and asserts the consumer receives the frames.
//
// Keeping it out of the shipped artifact matters. A capture library has no
// business being able to synthesise video.

#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>

#define TS_W 320
#define TS_H 240

typedef struct {
    struct pw_thread_loop *loop;
    struct pw_context *ctx;
    struct pw_core *core;
    struct pw_stream *stream;
    struct spa_hook listener;
    uint32_t node_id;
    uint8_t tick;
} TestSrc;

void miru_testsrc_stop(TestSrc *t);

static void ts_process(void *data) {
    TestSrc *t = data;
    struct pw_buffer *b = pw_stream_dequeue_buffer(t->stream);
    if (!b) return;
    struct spa_data *d = &b->buffer->datas[0];
    if (d->data) {
        size_t n = (size_t)TS_W * TS_H * 4;
        if (n > d->maxsize) n = d->maxsize;
        // A changing value per frame, so a consumer can tell frames apart.
        memset(d->data, t->tick++, n);
        d->chunk->offset = 0;
        d->chunk->stride = TS_W * 4;
        d->chunk->size = (uint32_t)n;
    }
    pw_stream_queue_buffer(t->stream, b);
}

/// Ask for buffers big enough for a whole frame. Without this the server picks
/// a small default and every "frame" is a truncated chunk, which would make the
/// consumer test far weaker than it looks.
static void ts_param_changed(void *data, uint32_t id, const struct spa_pod *param) {
    TestSrc *t = data;
    if (!param || id != SPA_PARAM_Format) return;
    uint8_t buf[512];
    struct spa_pod_builder pb = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
    const struct spa_pod *params[1];
    params[0] = spa_pod_builder_add_object(&pb,
        SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_blocks,  SPA_POD_Int(1),
        SPA_PARAM_BUFFERS_size,    SPA_POD_Int(TS_W * TS_H * 4),
        SPA_PARAM_BUFFERS_stride,  SPA_POD_Int(TS_W * 4));
    pw_stream_update_params(t->stream, params, 1);
}

static const struct pw_stream_events ts_events = {
    PW_VERSION_STREAM_EVENTS,
    .param_changed = ts_param_changed,
    .process = ts_process,
};

TestSrc *miru_testsrc_start(void) {
    pw_init(NULL, NULL);
    TestSrc *t = calloc(1, sizeof(TestSrc));
    if (!t) return NULL;
    t->loop = pw_thread_loop_new("miru-testsrc", NULL);
    if (!t->loop) { miru_testsrc_stop(t); return NULL; }
    pw_thread_loop_lock(t->loop);
    t->ctx = pw_context_new(pw_thread_loop_get_loop(t->loop), NULL, 0);
    if (!t->ctx) goto fail;
    t->core = pw_context_connect(t->ctx, NULL, 0);
    if (!t->core) goto fail;

    t->stream = pw_stream_new(t->core, "miru-testsrc",
        pw_properties_new(PW_KEY_MEDIA_TYPE, "Video",
                          PW_KEY_MEDIA_CATEGORY, "Playback",
                          PW_KEY_MEDIA_CLASS, "Video/Source",
                          PW_KEY_MEDIA_ROLE, "Screen", NULL));
    if (!t->stream) goto fail;
    pw_stream_add_listener(t->stream, &t->listener, &ts_events, t);

    uint8_t pod_buf[1024];
    struct spa_pod_builder pb = SPA_POD_BUILDER_INIT(pod_buf, sizeof(pod_buf));
    const struct spa_pod *params[1];
    params[0] = spa_pod_builder_add_object(&pb,
        SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
        SPA_FORMAT_mediaType,    SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format, SPA_POD_Id(SPA_VIDEO_FORMAT_BGRx),
        SPA_FORMAT_VIDEO_size,      SPA_POD_Rectangle(&SPA_RECTANGLE(TS_W, TS_H)),
        SPA_FORMAT_VIDEO_framerate, SPA_POD_Fraction(&SPA_FRACTION(30, 1)));

    // No DRIVER flag: let the graph's own driver clock this stream. As a
    // driver it would have to supply its own timing, and nothing would call
    // ts_process at all.
    if (pw_stream_connect(t->stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
                          PW_STREAM_FLAG_MAP_BUFFERS, params, 1) < 0) goto fail;

    pw_thread_loop_unlock(t->loop);
    if (pw_thread_loop_start(t->loop) < 0) { miru_testsrc_stop(t); return NULL; }

    // The node id is only assigned once the server has seen the stream.
    for (int i = 0; i < 100 && t->node_id == 0; i++) {
        pw_thread_loop_lock(t->loop);
        t->node_id = pw_stream_get_node_id(t->stream);
        pw_thread_loop_unlock(t->loop);
        if (t->node_id == 0 || t->node_id == PW_ID_ANY) { t->node_id = 0; usleep(20000); }
    }
    return t;

fail:
    pw_thread_loop_unlock(t->loop);
    miru_testsrc_stop(t);
    return NULL;
}

unsigned miru_testsrc_node_id(TestSrc *t) { return t ? t->node_id : 0; }
int      miru_testsrc_width(void)  { return TS_W; }
int      miru_testsrc_height(void) { return TS_H; }

void miru_testsrc_stop(TestSrc *t) {
    if (!t) return;
    if (t->loop) pw_thread_loop_stop(t->loop);
    if (t->stream) pw_stream_destroy(t->stream);
    if (t->core) pw_core_disconnect(t->core);
    if (t->ctx) pw_context_destroy(t->ctx);
    if (t->loop) pw_thread_loop_destroy(t->loop);
    free(t);
}
