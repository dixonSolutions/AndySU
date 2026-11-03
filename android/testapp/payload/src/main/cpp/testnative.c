#include <jni.h>
#include <setjmp.h>
#include <signal.h>
#include <unistd.h>

int dep_instances(void);
int dep_version(void);

JNIEXPORT jint JNICALL Java_com_andysu_testpayload_Native_depVersion(JNIEnv *e, jclass c) { return dep_version(); }
JNIEXPORT jint JNICALL Java_com_andysu_testpayload_Native_depInstances(JNIEnv *e, jclass c) { return dep_instances(); }
JNIEXPORT jint JNICALL Java_com_andysu_testpayload_Native_pageSize(JNIEnv *e, jclass c) { return (jint)sysconf(_SC_PAGESIZE); }
JNIEXPORT void JNICALL Java_com_andysu_testpayload_Native_crash(JNIEnv *e, jclass c) {
    volatile int *p = 0;
    *p = 1;
}

/* Like Xonotic's Sys_InitSignals: replaces the host's crash handlers with one that never returns. */
static void hang_handler(int sig) {
    for (;;) sleep(1);
}

JNIEXPORT void JNICALL Java_com_andysu_testpayload_Native_installOwnHandler(JNIEnv *e, jclass c) {
    struct sigaction sa;
    sa.sa_handler = hang_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    sigaction(SIGSEGV, &sa, NULL);
}

/* 1 while our hanging handler is still the installed SIGSEGV handler (a takeover host replaces it). */
JNIEXPORT jint JNICALL Java_com_andysu_testpayload_Native_ownHandlerActive(JNIEnv *e, jclass c) {
    struct sigaction cur;
    if (sigaction(SIGSEGV, NULL, &cur) != 0) return 0;
    return !(cur.sa_flags & SA_SIGINFO) && cur.sa_handler == hang_handler;
}

/* Like Mono: SIGSEGV is part of normal operation. A handler that turns an armed fault into a return
 * (siglongjmp) and chains to the previous handler for any other fault. */
static struct sigaction g_prev_segv;
static sigjmp_buf g_jb;
static volatile int g_armed;

static void handling_handler(int sig, siginfo_t *info, void *uc) {
    if (g_armed) siglongjmp(g_jb, 1);
    if (g_prev_segv.sa_flags & SA_SIGINFO) {
        if (g_prev_segv.sa_sigaction) g_prev_segv.sa_sigaction(sig, info, uc);
    } else if (g_prev_segv.sa_handler != SIG_DFL && g_prev_segv.sa_handler != SIG_IGN) {
        g_prev_segv.sa_handler(sig);
    }
    signal(sig, SIG_DFL);
}

JNIEXPORT void JNICALL Java_com_andysu_testpayload_Native_installHandlingHandler(JNIEnv *e, jclass c) {
    struct sigaction sa;
    sa.sa_sigaction = handling_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_SIGINFO | SA_NODEFER | SA_ONSTACK;
    sigaction(SIGSEGV, &sa, &g_prev_segv);
}

/* Faults deliberately; returns 1 when the handler resumed us. */
JNIEXPORT jint JNICALL Java_com_andysu_testpayload_Native_faultHandled(JNIEnv *e, jclass c) {
    int handled = 0;
    g_armed = 1;
    if (sigsetjmp(g_jb, 1) == 0) {
        volatile int *p = 0;
        *p = 1;
    } else {
        handled = 1;
    }
    g_armed = 0;
    return handled;
}
