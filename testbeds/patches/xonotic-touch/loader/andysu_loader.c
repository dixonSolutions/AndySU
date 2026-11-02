/*
 * libandysu_xonotic_loader.so: the resident part of the Xonotic Touch live engine swap.
 *
 * SDLActivity runs `AndySU_SDL_main` (XonoticActivity.getMainFunction) on the SDL thread instead
 * of the engine's SDL_main. This library loads the engine library (libmain[.<hash8>].so, found next
 * to this file) RTLD_LOCAL, resolves its single exported symbol andysu_engine_v1, and runs it.
 * SDL itself (libSDL2.so) stays one shared copy, so SDL's state is common to every engine copy.
 *
 * Stage 1: load and run once. No swapping. The loader also owns the JNI entry the Java side used
 * to send to the engine (nativeInputDevices) because the engine library is no longer loaded by
 * System.loadLibrary; the values are remembered and replayed into every engine that starts.
 */
#include <android/log.h>
#include <dirent.h>
#include <dlfcn.h>
#include <jni.h>
#include <stdatomic.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "andysu_swap.h"

#define EVENT_TAG "AndySU-Event"
#define TAG "AndySU-Loader"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

static const andysu_engine *g_engine;
/* nativeInputDevices state: bit 3 = the activity has reported at all. */
static atomic_int g_input_seen;
static atomic_int g_input_bits;

static void emit_json(const char *json) { __android_log_write(ANDROID_LOG_INFO, EVENT_TAG, json); }
static int api_swap_requested(void) { return 0; }
static void api_put_state(const char *p, size_t n) { (void)p; (void)n; }
static const char *api_get_state(size_t *n) { if (n) *n = 0; return NULL; }
static void api_mark_healthy(void) { LOGI("mark_healthy (stage 1: no host hook yet)"); }

static const andysu_loader_api g_api = {
    ANDYSU_ENGINE_ABI, sizeof(andysu_loader_api), api_swap_requested, api_put_state, api_get_state,
    api_mark_healthy, emit_json, NULL,
};

/* JNI: io.github.dixonsolutions.xonotictouch.XonoticActivity.nativeInputDevices (static native). */
JNIEXPORT void JNICALL Java_io_github_dixonsolutions_xonotictouch_XonoticActivity_nativeInputDevices(
    JNIEnv *env, jclass cls, jboolean touch, jboolean keyboard, jboolean mouse) {
    (void)env;
    (void)cls;
    atomic_store(&g_input_bits, (touch ? 1 : 0) | (keyboard ? 1 : 0) << 1 | (mouse ? 1 : 0) << 2);
    atomic_store(&g_input_seen, 1);
    if (g_engine && g_engine->input_devices) g_engine->input_devices(touch, keyboard, mouse);
}

/* The engine library sits next to this file: libmain.so, or libmain.<hash8>.so after renaming. */
static int find_engine(char *out, size_t cap) {
    Dl_info di;
    if (!dladdr((void *)find_engine, &di) || !di.dli_fname) return -1;
    char dir[512];
    snprintf(dir, sizeof dir, "%s", di.dli_fname);
    char *slash = strrchr(dir, '/');
    if (!slash) return -1;
    *slash = 0;
    snprintf(out, cap, "%s/libmain.so", dir);
    if (access(out, R_OK) == 0) return 0;
    DIR *d = opendir(dir);
    if (!d) return -1;
    struct dirent *e;
    int found = -1;
    while ((e = readdir(d))) {
        size_t n = strlen(e->d_name);
        if (!strncmp(e->d_name, "libmain.", 8) && n > 11 && !strcmp(e->d_name + n - 3, ".so")) {
            snprintf(out, cap, "%s/%s", dir, e->d_name);
            found = 0;
            break;
        }
    }
    closedir(d);
    return found;
}

static void emit_event(const char *event, const char *lib, const char *extra) {
    char buf[1024];
    snprintf(buf, sizeof buf, "{\"event\":\"%s\",\"pid\":%d,\"lib\":\"%s\"%s%s}", event, (int)getpid(), lib ? lib : "",
             extra ? "," : "", extra ? extra : "");
    emit_json(buf);
}

/* SDLActivity: nativeRunMain(library = this file, function = "AndySU_SDL_main"). */
__attribute__((visibility("default"))) int AndySU_SDL_main(int argc, char *argv[]) {
    char path[600];
    if (find_engine(path, sizeof path)) {
        LOGE("no libmain*.so next to the loader");
        emit_event("loader_failed", "", "\"error\":\"engine library not found\"");
        return 1;
    }
    void *h = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (!h) {
        LOGE("dlopen %s: %s", path, dlerror());
        emit_event("loader_failed", path, "\"error\":\"dlopen failed\"");
        return 1;
    }
    andysu_engine_fn fn = (andysu_engine_fn)dlsym(h, ANDYSU_ENGINE_SYMBOL);
    const andysu_engine *e = fn ? fn() : NULL;
    if (!e || e->abi != ANDYSU_ENGINE_ABI || e->size < sizeof(andysu_engine) || !e->run) {
        LOGE("%s: missing or incompatible %s", path, ANDYSU_ENGINE_SYMBOL);
        emit_event("loader_failed", path, "\"error\":\"bad engine entry\"");
        return 1;
    }
    g_engine = e;
    char extra[256];
    snprintf(extra, sizeof extra, "\"build_id\":\"%s\",\"swapped\":0", e->build_id ? e->build_id : "");
    emit_event("loader_engine_loaded", path, extra);
    if (atomic_load(&g_input_seen) && e->input_devices) {
        int b = atomic_load(&g_input_bits);
        e->input_devices(b & 1, (b >> 1) & 1, (b >> 2) & 1);
    }
    int rc = e->run(&g_api, argc, argv, 0);
    snprintf(extra, sizeof extra, "\"rc\":%d", rc);
    emit_event("loader_engine_returned", path, extra);
    return rc;
}
