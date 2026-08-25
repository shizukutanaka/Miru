// xdg-desktop-portal ScreenCast negotiation, and the PipeWire stream it hands
// back, behind a small C surface.
//
// WHY C, AND WHY NO BINDING CRATE
//
// Same reasoning as crates/miru-codec/csrc/miru_ffmpeg.c: these are C libraries
// with stable APIs, and the hard part of binding them from Rust is struct
// layout and callback ABI, not logic. Keeping GVariant, pw_stream and spa_pod
// on the C side means no layout assumption crosses into Rust, and the Wayland
// capture path needs no crates.io dependency.
//
// THE PORTAL DANCE
//
// Every portal call returns a Request object path and answers asynchronously on
// that object's Response signal. The sequence is fixed:
//   CreateSession -> SelectSources -> Start -> OpenPipeWireRemote
// Start is the one that may prompt the user; it returns the node id to stream.
// We run a private GMainContext so this never depends on the caller having one.

#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>
#include <spa/debug/types.h>
#include <string.h>
#include <stdlib.h>

// ── Portal ───────────────────────────────────────────────────────────────────

typedef struct {
    GMainContext *ctx;
    GMainLoop *loop;
    GDBusConnection *bus;
    char *session_handle;
    guint32 node_id;
    int pw_fd;
    int failed;
    /// Which step failed. Callers surface this: "the portal refused" is not an
    /// actionable message, "SelectSources was refused" is.
    const char *stage;
    /// The D-Bus error text from that step, if there was one.
    char *detail;
} Portal;

typedef struct {
    Portal *p;
    guint sub;
    GVariant *result;   // owned
    guint32 response;   // 0 = success
    int done;
} ReqWait;

static void on_response(GDBusConnection *c, const char *sender, const char *path,
                        const char *iface, const char *sig, GVariant *params,
                        gpointer user) {
    (void)c; (void)sender; (void)path; (void)iface; (void)sig;
    ReqWait *w = user;
    g_variant_get(params, "(u@a{sv})", &w->response, &w->result);
    w->done = 1;
    g_main_loop_quit(w->p->loop);
}

/// Unique token for a Request/Session handle, as the portal spec requires.
static char *new_token(const char *prefix) {
    static guint counter = 0;
    return g_strdup_printf("miru_%s_%u_%u", prefix, (guint)getpid(), counter++);
}

/// Subscribe to a Request's Response before making the call, so the reply
/// cannot be missed by racing the signal.
static ReqWait *wait_begin(Portal *p, const char *token) {
    char *sender = g_strdup(g_dbus_connection_get_unique_name(p->bus) + 1);
    for (char *s = sender; *s; s++) if (*s == '.') *s = '_';
    char *path = g_strdup_printf("/org/freedesktop/portal/desktop/request/%s/%s",
                                 sender, token);
    ReqWait *w = g_new0(ReqWait, 1);
    w->p = p;
    w->sub = g_dbus_connection_signal_subscribe(
        p->bus, "org.freedesktop.portal.Desktop", "org.freedesktop.portal.Request",
        "Response", path, NULL, G_DBUS_SIGNAL_FLAGS_NONE, on_response, w, NULL);
    g_free(path);
    g_free(sender);
    return w;
}

static gboolean quit_loop(gpointer loop) {
    g_main_loop_quit(loop);
    return G_SOURCE_REMOVE;
}

/// Returns 0 on a successful (response == 0) reply, non-zero otherwise.
static int wait_finish(Portal *p, ReqWait *w, GVariant **out, int timeout_ms) {
    guint tid = g_timeout_add(timeout_ms, quit_loop, p->loop);
    if (!w->done) g_main_loop_run(p->loop);
    g_source_remove(tid);
    if (!w->done) {
        g_free(p->detail);
        p->detail = g_strdup("no Response signal before the timeout");
    } else if (w->response != 0) {
        g_free(p->detail);
        // 1 = the user cancelled, 2 = ended for another reason.
        p->detail = g_strdup_printf("portal answered response=%u (%s)", w->response,
                                    w->response == 1 ? "cancelled by user/backend"
                                                     : "ended by the backend");
    }
    int rc = (w->done && w->response == 0) ? 0 : -1;
    if (out && w->result) *out = g_variant_ref(w->result);
    if (w->result) g_variant_unref(w->result);
    g_dbus_connection_signal_unsubscribe(p->bus, w->sub);
    g_free(w);
    return rc;
}

static GVariant *portal_call(Portal *p, const char *method, GVariant *args) {
    GError *err = NULL;
    GVariant *r = g_dbus_connection_call_sync(
        p->bus, "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast", method, args, G_VARIANT_TYPE("(o)"),
        G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &err);
    if (err) {
        g_free(p->detail);
        p->detail = g_strdup(err->message);
        g_error_free(err);
        return NULL;
    }
    return r;
}

Portal *miru_portal_open(int include_cursor) {
    pw_init(NULL, NULL);
    Portal *p = g_new0(Portal, 1);
    p->pw_fd = -1;
    p->ctx = g_main_context_new();
    g_main_context_push_thread_default(p->ctx);
    p->loop = g_main_loop_new(p->ctx, FALSE);

    GError *err = NULL;
    p->stage = "connect to the session bus";
    p->bus = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &err);
    if (!p->bus) { if (err) g_error_free(err); goto fail; }

    // ── CreateSession ────────────────────────────────────────────────────────
    p->stage = "CreateSession";
    char *rtok = new_token("req");
    char *stok = new_token("sess");
    ReqWait *w = wait_begin(p, rtok);
    GVariantBuilder b;
    g_variant_builder_init(&b, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&b, "{sv}", "handle_token", g_variant_new_string(rtok));
    g_variant_builder_add(&b, "{sv}", "session_handle_token", g_variant_new_string(stok));
    GVariant *ret = portal_call(p, "CreateSession", g_variant_new("(a{sv})", &b));
    g_free(rtok); g_free(stok);
    if (!ret) { wait_finish(p, w, NULL, 1); goto fail; }
    g_variant_unref(ret);

    GVariant *res = NULL;
    if (wait_finish(p, w, &res, 5000) != 0) { if (res) g_variant_unref(res); goto fail; }
    g_variant_lookup(res, "session_handle", "s", &p->session_handle);
    g_variant_unref(res);
    if (!p->session_handle) goto fail;

    // ── SelectSources ────────────────────────────────────────────────────────
    p->stage = "SelectSources";
    rtok = new_token("req");
    w = wait_begin(p, rtok);
    g_variant_builder_init(&b, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&b, "{sv}", "handle_token", g_variant_new_string(rtok));
    g_variant_builder_add(&b, "{sv}", "types", g_variant_new_uint32(1)); // MONITOR
    g_variant_builder_add(&b, "{sv}", "multiple", g_variant_new_boolean(FALSE));
    g_variant_builder_add(&b, "{sv}", "cursor_mode",
                          g_variant_new_uint32(include_cursor ? 2 : 1)); // embedded : hidden
    ret = portal_call(p, "SelectSources",
                      g_variant_new("(oa{sv})", p->session_handle, &b));
    g_free(rtok);
    if (!ret) { wait_finish(p, w, NULL, 1); goto fail; }
    g_variant_unref(ret);
    if (wait_finish(p, w, NULL, 5000) != 0) goto fail;

    // ── Start ────────────────────────────────────────────────────────────────
    p->stage = "Start";
    rtok = new_token("req");
    w = wait_begin(p, rtok);
    g_variant_builder_init(&b, G_VARIANT_TYPE_VARDICT);
    g_variant_builder_add(&b, "{sv}", "handle_token", g_variant_new_string(rtok));
    ret = portal_call(p, "Start",
                      g_variant_new("(osa{sv})", p->session_handle, "", &b));
    g_free(rtok);
    if (!ret) { wait_finish(p, w, NULL, 1); goto fail; }
    g_variant_unref(ret);
    // Generous: this is where a compositor may show a picker.
    if (wait_finish(p, w, &res, 60000) != 0) { if (res) g_variant_unref(res); goto fail; }

    GVariant *streams = g_variant_lookup_value(res, "streams", G_VARIANT_TYPE("a(ua{sv})"));
    g_variant_unref(res);
    if (!streams || g_variant_n_children(streams) == 0) {
        if (streams) g_variant_unref(streams);
        goto fail;
    }
    GVariant *first = g_variant_get_child_value(streams, 0);
    GVariant *props = NULL;
    g_variant_get(first, "(u@a{sv})", &p->node_id, &props);
    if (props) g_variant_unref(props);
    g_variant_unref(first);
    g_variant_unref(streams);

    // ── OpenPipeWireRemote ───────────────────────────────────────────────────
    p->stage = "OpenPipeWireRemote";
    g_variant_builder_init(&b, G_VARIANT_TYPE_VARDICT);
    GUnixFDList *fds = NULL;
    ret = g_dbus_connection_call_with_unix_fd_list_sync(
        p->bus, "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast", "OpenPipeWireRemote",
        g_variant_new("(oa{sv})", p->session_handle, &b), G_VARIANT_TYPE("(h)"),
        G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &fds, NULL, &err);
    if (err) { g_error_free(err); goto fail; }
    if (ret) {
        gint32 idx = -1;
        g_variant_get(ret, "(h)", &idx);
        g_variant_unref(ret);
        if (fds && idx >= 0) p->pw_fd = g_unix_fd_list_get(fds, idx, NULL);
    }
    if (fds) g_object_unref(fds);
    if (p->pw_fd < 0) goto fail;

    p->stage = NULL;
    g_main_context_pop_thread_default(p->ctx);
    return p;

fail:
    p->failed = 1;
    g_main_context_pop_thread_default(p->ctx);
    return p;
}

int      miru_portal_ok(Portal *p)      { return p && !p->failed; }
/// NULL when the handshake succeeded, otherwise the step that refused.
const char *miru_portal_stage(Portal *p) { return p ? p->stage : "allocate"; }
/// D-Bus error text for the failing step, or NULL.
const char *miru_portal_detail(Portal *p) { return p ? p->detail : NULL; }
unsigned miru_portal_node_id(Portal *p) { return p ? p->node_id : 0; }
int      miru_portal_fd(Portal *p)      { return p ? p->pw_fd : -1; }

void miru_portal_close(Portal *p) {
    if (!p) return;
    if (p->session_handle && p->bus) {
        g_dbus_connection_call_sync(
            p->bus, "org.freedesktop.portal.Desktop", p->session_handle,
            "org.freedesktop.portal.Session", "Close", NULL, NULL,
            G_DBUS_CALL_FLAGS_NONE, 2000, NULL, NULL);
        g_free(p->session_handle);
    }
    g_free(p->detail);
    if (p->pw_fd >= 0) close(p->pw_fd);
    if (p->bus) g_object_unref(p->bus);
    if (p->loop) g_main_loop_unref(p->loop);
    if (p->ctx) g_main_context_unref(p->ctx);
    g_free(p);
}
