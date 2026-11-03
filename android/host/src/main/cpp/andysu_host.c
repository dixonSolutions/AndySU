/*
 * AndySU Android host native bridge (libandysu_host.so).
 *
 *  - Loads a native payload generation: every library by absolute path, dependencies first, so
 *    renamed DT_NEEDED entries resolve to the copies of the same generation.
 *  - Calls the payload's start/frame/suspend with an andysu_host whose callbacks reach Java.
 *  - Crash handler: writes the core's crash marker with async-signal-safe calls. On a non-main
 *    thread it also pokes a pipe watched from Java (which relaunches cleanly through the phoenix
 *    activity) and parks the crashing thread; on the main thread it chains to the previous
 *    handler (the process dies; the next launch rolls back).
 *  - If a payload replaces these handlers (D30), check_handlers() reports it; with crash_signals =
 *    "host" it also re-installs ours and keeps the payload's as a chained handler, called after
 *    the marker is written.
 */
#include <android/log.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <jni.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "andysu_payload.h"

#define TAG "AndySU"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

typedef struct gen {
    void *handles[64];
    int nh;
    const andysu_payload *p;
    andysu_host host;
    char slot_dir[512];
    char version[96];
} gen;

static JavaVM *g_vm;
static jclass g_bridge;            /* com.andysu.host.NativeBridge (global ref) */
static jmethodID g_on_healthy, g_on_log, g_on_emit, g_on_check;
static char g_err[512];
static void check_after_start(void); /* crash handling, below */

/* A missing callback must not take the host down: log it and leave that callback unset. */
static jmethodID static_method(JNIEnv *env, jclass c, const char *name, const char *sig) {
    jmethodID m = (*env)->GetStaticMethodID(env, c, name, sig);
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionClear(env);
        LOGE("NativeBridge.%s%s not found; payload callback disabled", name, sig);
        return NULL;
    }
    return m;
}

JNIEXPORT jint JNI_OnLoad(JavaVM *vm, void *reserved) {
    (void)reserved;
    g_vm = vm;
    JNIEnv *env;
    if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
    jclass c = (*env)->FindClass(env, "com/andysu/host/NativeBridge");
    if (!c) return JNI_ERR;
    g_bridge = (*env)->NewGlobalRef(env, c);
    g_on_healthy = static_method(env, c, "onMarkHealthy", "()V");
    g_on_log = static_method(env, c, "onLog", "(Ljava/lang/String;)V");
    g_on_emit = static_method(env, c, "onEmit", "(Ljava/lang/String;)V");
    g_on_check = static_method(env, c, "onRequestCheck", "()V");
    return JNI_VERSION_1_6;
}

static JNIEnv *env_for_thread(void) {
    JNIEnv *env = NULL;
    if ((*g_vm)->GetEnv(g_vm, (void **)&env, JNI_VERSION_1_6) == JNI_EDETACHED)
        (*g_vm)->AttachCurrentThread(g_vm, &env, NULL);
    return env;
}

static void call_void(jmethodID m) {
    JNIEnv *env = env_for_thread();
    if (env && m) (*env)->CallStaticVoidMethod(env, g_bridge, m);
}

static void call_str(jmethodID m, const char *s) {
    JNIEnv *env = env_for_thread();
    if (!env || !m) return;
    jstring j = (*env)->NewStringUTF(env, s ? s : "");
    (*env)->CallStaticVoidMethod(env, g_bridge, m, j);
    (*env)->DeleteLocalRef(env, j);
}

static void cb_healthy(void) { call_void(g_on_healthy); }
static void cb_log(const char *m) { call_str(g_on_log, m); }
static void cb_emit(const char *j) { call_str(g_on_emit, j); }
static void cb_check(void) { call_void(g_on_check); }

JNIEXPORT jstring JNICALL Java_com_andysu_host_NativeBridge_lastError(JNIEnv *env, jclass c) {
    (void)c;
    return (*env)->NewStringUTF(env, g_err);
}

JNIEXPORT jlong JNICALL Java_com_andysu_host_NativeBridge_load(JNIEnv *env, jclass c, jobjectArray libs,
                                                               jstring entry, jstring symbol, jstring slot,
                                                               jstring version) {
    (void)c;
    g_err[0] = 0;
    gen *g = calloc(1, sizeof *g);
    const char *e = (*env)->GetStringUTFChars(env, entry, NULL);
    const char *sym = (*env)->GetStringUTFChars(env, symbol, NULL);
    void *entry_h = NULL;
    jsize n = (*env)->GetArrayLength(env, libs);
    for (jsize i = 0; i < n && g->nh < 64; i++) {
        jstring js = (jstring)(*env)->GetObjectArrayElement(env, libs, i);
        const char *path = (*env)->GetStringUTFChars(env, js, NULL);
        void *h = dlopen(path, RTLD_NOW | RTLD_LOCAL);
        if (!h) {
            snprintf(g_err, sizeof g_err, "dlopen %s: %s", path, dlerror());
            LOGE("%s", g_err);
        } else {
            g->handles[g->nh++] = h;
            if (!strcmp(path, e)) entry_h = h;
        }
        (*env)->ReleaseStringUTFChars(env, js, path);
        (*env)->DeleteLocalRef(env, js);
        if (!h) break;
    }
    const andysu_payload *(*fn)(void) = NULL;
    if (!g_err[0]) {
        if (entry_h) fn = (const andysu_payload *(*)(void))dlsym(entry_h, sym);
        if (!fn || !(g->p = fn()) || !g->p->start)
            snprintf(g_err, sizeof g_err, "entry %s missing or invalid in %s", sym, e);
    }
    (*env)->ReleaseStringUTFChars(env, entry, e);
    (*env)->ReleaseStringUTFChars(env, symbol, sym);
    if (g_err[0]) {
        free(g); /* libraries stay mapped; dlclose is unreliable for this use anyway */
        return 0;
    }
    const char *sd = (*env)->GetStringUTFChars(env, slot, NULL);
    const char *ver = (*env)->GetStringUTFChars(env, version, NULL);
    snprintf(g->slot_dir, sizeof g->slot_dir, "%s", sd);
    snprintf(g->version, sizeof g->version, "%s", ver);
    (*env)->ReleaseStringUTFChars(env, slot, sd);
    (*env)->ReleaseStringUTFChars(env, version, ver);
    andysu_host *h = &g->host;
    h->api_level = ANDYSU_HOST_API_LEVEL;
    h->struct_size = sizeof *h;
    h->slot_dir = g->slot_dir;
    h->version = g->version;
    h->log = cb_log;
    h->mark_healthy = cb_healthy;
    h->emit = cb_emit;
    h->request_update_check = cb_check;
    h->java_vm = g_vm;
    return (jlong)(intptr_t)g;
}

JNIEXPORT jint JNICALL Java_com_andysu_host_NativeBridge_start(JNIEnv *env, jclass c, jlong handle, jbyteArray state,
                                                               jobject activity, jint w, jint h) {
    (void)c;
    gen *g = (gen *)(intptr_t)handle;
    if (g->host.activity) (*env)->DeleteGlobalRef(env, g->host.activity);
    g->host.activity = activity ? (*env)->NewGlobalRef(env, activity) : NULL;
    g->host.surface_width = w;
    g->host.surface_height = h;
    jbyte *b = NULL;
    jsize len = 0;
    if (state) {
        len = (*env)->GetArrayLength(env, state);
        b = (*env)->GetByteArrayElements(env, state, NULL);
    }
    int rc = g->p->start(&g->host, (const char *)b, (size_t)len);
    if (b) (*env)->ReleaseByteArrayElements(env, state, b, JNI_ABORT);
    check_after_start();
    return rc;
}

JNIEXPORT jint JNICALL Java_com_andysu_host_NativeBridge_frame(JNIEnv *env, jclass c, jlong handle, jint w, jint h) {
    (void)env;
    (void)c;
    gen *g = (gen *)(intptr_t)handle;
    g->host.surface_width = w;
    g->host.surface_height = h;
    return g->p->frame ? g->p->frame() : 0;
}

JNIEXPORT jbyteArray JNICALL Java_com_andysu_host_NativeBridge_suspend(JNIEnv *env, jclass c, jlong handle) {
    (void)c;
    gen *g = (gen *)(intptr_t)handle;
    if (!g->p->suspend) return NULL;
    size_t len = 0;
    char *s = g->p->suspend(&len);
    if (!s) return NULL;
    jbyteArray a = (*env)->NewByteArray(env, (jsize)len);
    (*env)->SetByteArrayRegion(env, a, 0, (jsize)len, (const jbyte *)s);
    free(s);
    return a;
}

/* ---------- crash handling ---------- */

static char g_marker[512];
static int g_notify_fd = -1;
static pid_t g_main_tid;
static volatile int g_relaunch; /* set by Java while a generation is on trial */
static struct sigaction g_old[32];
static struct sigaction g_host_sa;
static const int g_sigs[] = {SIGSEGV, SIGABRT, SIGBUS, SIGFPE, SIGILL, SIGTRAP};
/* A handler a payload installed over ours (D30), kept to call after the marker is written. Double
 * buffered: the checker fills the idle slot, then flips g_chain_idx, so a handler never sees a torn entry. */
static struct sigaction g_chain[32][2];
static volatile int g_chain_idx[32];
static volatile int g_chain_valid[32];
static volatile int g_in_chain[32]; /* a payload handler that calls back into ours must not recurse */

static const char *sig_name(int sig) {
    return sig == SIGSEGV ? "SIGSEGV" : sig == SIGABRT ? "SIGABRT" : sig == SIGBUS ? "SIGBUS"
         : sig == SIGFPE ? "SIGFPE" : sig == SIGILL ? "SIGILL" : "SIGTRAP";
}

static void on_crash(int sig, siginfo_t *info, void *uc) {
    int fd = open(g_marker, O_CREAT | O_WRONLY | O_TRUNC, 0600);
    if (fd >= 0) {
        const char *n = sig_name(sig);
        ssize_t w = write(fd, n, strlen(n));
        (void)w;
        fsync(fd);
        close(fd);
    }
    pid_t tid = (pid_t)syscall(__NR_gettid);
    const struct sigaction *chain = NULL;
    if (sig > 0 && sig < 32 && g_chain_valid[sig] && !g_in_chain[sig]) chain = &g_chain[sig][g_chain_idx[sig]];
    int poked = 0;
    /* Poke Java first: a payload handler (an engine's crash dialog) may never return, and the
     * process has to be relaunched from outside the crashing thread. */
    if (g_relaunch && g_notify_fd >= 0 && (tid != g_main_tid || chain)) {
        char b = (char)sig;
        ssize_t w = write(g_notify_fd, &b, 1);
        (void)w;
        poked = 1;
    }
    if (chain) {
        g_in_chain[sig] = 1;
        if (chain->sa_flags & SA_SIGINFO) {
            if (chain->sa_sigaction) chain->sa_sigaction(sig, info, uc);
        } else if (chain->sa_handler != SIG_DFL && chain->sa_handler != SIG_IGN) {
            chain->sa_handler(sig);
        }
        g_in_chain[sig] = 0;
    }
    if (poked) {
        /* Park this thread; Java relaunches the process. If that never happens, die normally. */
        struct timespec ts = {5, 0};
        while (nanosleep(&ts, &ts) == -1 && errno == EINTR) {}
    }
    /* Chain to the previous handler (debuggerd), then the default action. */
    struct sigaction *old = &g_old[sig];
    sigaction(sig, old, NULL);
    if (old->sa_flags & SA_SIGINFO) {
        if (old->sa_sigaction) old->sa_sigaction(sig, info, uc);
    } else if (old->sa_handler != SIG_DFL && old->sa_handler != SIG_IGN) {
        old->sa_handler(sig);
    }
    signal(sig, SIG_DFL);
    raise(sig);
}

/* A payload that replaced the host's handlers is always reported (crash_handler_replaced, once per
 * handler). With takeover ("crash_signals": "host") the host also re-installs its own handler and
 * keeps the payload's as the chained one; otherwise nothing changes: runtimes such as Mono use
 * SIGSEGV for normal operation and must keep their handler first (D30). Returns how many signals
 * were reported. */
static int g_takeover;
static void *g_reported[32];
static int g_reported_set[32];

static int check_handlers(int takeover) {
    char list[160] = "";
    int n = 0;
    g_takeover = takeover;
    for (size_t i = 0; i < sizeof g_sigs / sizeof *g_sigs; i++) {
        int sig = g_sigs[i];
        struct sigaction cur;
        if (sigaction(sig, NULL, &cur) != 0) continue;
        if ((cur.sa_flags & SA_SIGINFO) && cur.sa_sigaction == on_crash) {
            g_reported_set[sig] = 0;
            continue;
        }
        if (takeover) {
            struct sigaction prev;
            if (sigaction(sig, &g_host_sa, &prev) != 0) continue;
            int real = (prev.sa_flags & SA_SIGINFO) ? prev.sa_sigaction != NULL
                                                    : (prev.sa_handler != SIG_DFL && prev.sa_handler != SIG_IGN);
            if (real) {
                int idle = 1 - g_chain_idx[sig];
                g_chain[sig][idle] = prev;
                g_chain_idx[sig] = idle;
                g_chain_valid[sig] = 1;
            } else {
                g_chain_valid[sig] = 0;
            }
        } else {
            void *h = (cur.sa_flags & SA_SIGINFO) ? (void *)cur.sa_sigaction : (void *)cur.sa_handler;
            if (g_reported_set[sig] && g_reported[sig] == h) continue;
            g_reported[sig] = h;
            g_reported_set[sig] = 1;
        }
        if (n++) strncat(list, ",", sizeof list - strlen(list) - 1);
        strncat(list, "\"", sizeof list - strlen(list) - 1);
        strncat(list, sig_name(sig), sizeof list - strlen(list) - 1);
        strncat(list, "\"", sizeof list - strlen(list) - 1);
    }
    if (n) {
        char json[256];
        snprintf(json, sizeof json, "{\"event\":\"crash_handler_replaced\",\"signals\":[%s],\"chained\":%s}", list,
                 takeover ? "true" : "false");
        LOGE("payload replaced the crash handlers (%s); %s", list,
             takeover ? "host handlers re-installed, payload's chained" : "reported only (crash_signals = payload)");
        cb_emit(json);
    }
    return n;
}

static void check_after_start(void) {
    if (g_marker[0]) check_handlers(g_takeover);
}

JNIEXPORT jint JNICALL Java_com_andysu_host_NativeBridge_checkCrashHandlers(JNIEnv *env, jclass c, jboolean takeover) {
    (void)env;
    (void)c;
    return g_marker[0] ? check_handlers(takeover ? 1 : 0) : 0;
}

JNIEXPORT void JNICALL Java_com_andysu_host_NativeBridge_installCrashHandler(JNIEnv *env, jclass c, jstring marker,
                                                                            jint notify_fd) {
    (void)c;
    const char *m = (*env)->GetStringUTFChars(env, marker, NULL);
    snprintf(g_marker, sizeof g_marker, "%s", m);
    (*env)->ReleaseStringUTFChars(env, marker, m);
    g_notify_fd = notify_fd;
    g_main_tid = getpid(); /* called on the main thread, whose tid equals the pid */
    struct sigaction *sa = &g_host_sa;
    memset(sa, 0, sizeof *sa);
    sa->sa_sigaction = on_crash;
    sa->sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&sa->sa_mask);
    for (size_t i = 0; i < sizeof g_sigs / sizeof *g_sigs; i++) sigaction(g_sigs[i], sa, &g_old[g_sigs[i]]);
    LOGI("crash handler installed (marker %s)", g_marker);
}

JNIEXPORT void JNICALL Java_com_andysu_host_NativeBridge_setRelaunchOnCrash(JNIEnv *env, jclass c, jboolean on) {
    (void)env;
    (void)c;
    g_relaunch = on ? 1 : 0;
}

/* Test hook: crash the calling thread (used by instrumented tests of the crash path). */
JNIEXPORT void JNICALL Java_com_andysu_host_NativeBridge_crashForTest(JNIEnv *env, jclass c) {
    (void)env;
    (void)c;
    volatile int *p = NULL;
    *p = 42;
}
