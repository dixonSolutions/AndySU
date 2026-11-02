/*
 * AndySU Linux desktop host.
 *
 * A small, frozen executable that boots native payloads through the AndySU core (C ABI), checks
 * a remote for signed updates, swaps payload generations live (suspend -> load new uniquely
 * named .so files -> resume with the same state), and rolls back:
 *   - live, without restarting, when the new generation's start() fails or it never calls
 *     mark_healthy() before the watchdog fires;
 *   - by relaunching itself once, when the new generation crashes (a signal handler writes the
 *     core's crash marker and re-executes the host; the next launch turns the marker into a
 *     rollback).
 *
 * It speaks JSON lines on stdout (events) and reads simple commands on stdin:
 *   check   check for updates now (applies live with --live)
 *   state   print the core state
 *   quit    suspend the payload and exit
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <time.h>
#include <unistd.h>

#include "andysu.h"
#include "andysu_payload.h"

#define MAX_LIBS 64

typedef struct gen {
    void *handles[MAX_LIBS];
    int nh;
    const andysu_payload *p;
    char dir[PATH_MAX];
    char version[96];
    int factory;
} gen;

static struct {
    const char *root, *url, *pub_path, *app, *channel, *abi, *factory;
    int live, max_attempts;
    const char *apply; /* live | restart | next_start (--live is apply=live) */
    long watchdog_ms, check_ms;
} O = {.channel = "stable", .abi = "x86_64", .max_attempts = 2, .watchdog_ms = 5000};

static char *g_pub_pem;
static char g_marker[PATH_MAX];
static char g_self[PATH_MAX];
static char **g_argv;
static gen *cur, *prev;
static andysu_host H;
static int in_trial, trial_live;
static long trial_deadline;
static char *swap_state;       /* state handed to the generation on trial (for live rollback) */
static size_t swap_state_len;
static int check_requested;
static int live_swaps;
static uint8_t r16[16];

/* ---------- small utilities ---------- */

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

static int64_t now_s(void) { return (int64_t)time(NULL); }

static void out(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vfprintf(stdout, fmt, ap);
    va_end(ap);
    fputc('\n', stdout);
    fflush(stdout);
}

/* Escape a string for JSON. Returns malloc'd. */
static char *jesc(const char *s) {
    size_t n = strlen(s);
    char *o = malloc(n * 6 + 3), *w = o;
    for (; *s; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') { *w++ = '\\'; *w++ = (char)c; }
        else if (c == '\n') { *w++ = '\\'; *w++ = 'n'; }
        else if (c < 0x20) w += sprintf(w, "\\u%04x", c);
        else *w++ = (char)c;
    }
    *w = 0;
    return o;
}

/* Find "key": in JSON text and return a pointer to its value (no full parser; keys used here are
 * unique in the documents the core returns). */
static const char *jfind(const char *j, const char *key) {
    char pat[128];
    snprintf(pat, sizeof pat, "\"%s\":", key);
    const char *p = j ? strstr(j, pat) : NULL;
    if (!p) return NULL;
    p += strlen(pat);
    while (*p == ' ' || *p == '\n') p++;
    return p;
}

/* Parse a JSON string starting at p (which points at '"'). Returns malloc'd, sets *end. */
static char *jparse_str(const char *p, const char **end) {
    if (!p || *p != '"') return NULL;
    p++;
    char *o = malloc(strlen(p) + 1), *w = o;
    while (*p && *p != '"') {
        if (*p == '\\' && p[1]) {
            p++;
            switch (*p) {
            case 'n': *w++ = '\n'; break;
            case 't': *w++ = '\t'; break;
            case 'u': *w++ = '?'; p += 4; break;
            default: *w++ = *p;
            }
            p++;
        } else
            *w++ = *p++;
    }
    *w = 0;
    if (end) *end = *p ? p + 1 : p;
    return o;
}

static char *jstr(const char *j, const char *key) { return jparse_str(jfind(j, key), NULL); }

static int jbool(const char *j, const char *key) {
    const char *p = jfind(j, key);
    return p && strncmp(p, "true", 4) == 0;
}

static int jerr(const char *j) { return j && strncmp(j, "{\"error\"", 8) == 0; }

/* Emit a core result as an event, then free it. Returns 0 on ok. */
static int report(const char *what, char *res) {
    int bad = jerr(res);
    out("{\"event\":\"%s\",\"result\":%s}", what, res ? res : "null");
    andysu_free(res);
    return bad ? -1 : 0;
}

/* ---------- crash handling ---------- */

extern char **environ;
static int g_relaunches;          /* parsed at startup: the handler must not call getenv */
static char g_env_relaunch[32];   /* "ANDYSU_RELAUNCHES=<n+1>", prepared at startup */
static char *g_envp[1024];

static void on_crash(int sig) {
    int fd = open(g_marker, O_CREAT | O_WRONLY | O_TRUNC, 0600);
    if (fd >= 0) {
        const char *n = sig == SIGSEGV ? "SIGSEGV" : sig == SIGABRT ? "SIGABRT" : sig == SIGBUS ? "SIGBUS"
                      : sig == SIGFPE ? "SIGFPE" : "SIGILL";
        ssize_t w = write(fd, n, strlen(n));
        (void)w;
        close(fd);
    }
    /* Relaunch at most a few times in a row (a committed slot is never rolled back, so do not
     * loop forever on one that keeps crashing). Only async-signal-safe calls from here on:
     * the environment for execve is assembled in static storage. */
    if (g_relaunches < 3) {
        size_t k = 0;
        for (char **e = environ; e && *e && k < sizeof g_envp / sizeof *g_envp - 2; e++)
            if (strncmp(*e, "ANDYSU_RELAUNCHES=", 18) != 0) g_envp[k++] = *e;
        g_envp[k++] = g_env_relaunch;
        g_envp[k] = NULL;
        execve(g_self, g_argv, g_envp);
    }
    signal(sig, SIG_DFL);
    raise(sig);
}

static void install_crash_handlers(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_crash;
    sa.sa_flags = SA_NODEFER | SA_RESETHAND;
    int sigs[] = {SIGSEGV, SIGABRT, SIGBUS, SIGFPE, SIGILL};
    for (size_t i = 0; i < sizeof sigs / sizeof *sigs; i++) sigaction(sigs[i], &sa, NULL);
    sigset_t all;
    sigemptyset(&all);
    sigprocmask(SIG_SETMASK, &all, NULL); /* exec from a handler keeps the mask; clear it */
}

static void relaunch(const char *why) {
    out("{\"event\":\"relaunch\",\"reason\":\"%s\"}", why);
    execv(g_self, g_argv);
    perror("execv");
    exit(3);
}

/* ---------- host API given to payloads ---------- */

static void h_log(const char *m) {
    char *e = jesc(m);
    out("{\"event\":\"log\",\"msg\":\"%s\"}", e);
    free(e);
}

static void h_emit(const char *json) { out("%s", json); }

static void h_mark_healthy(void) {
    if (!in_trial) return;
    in_trial = 0;
    free(swap_state);
    swap_state = NULL;
    report("committed", andysu_mark_healthy(O.root));
}

static void h_request_check(void) { check_requested = 1; }

/* ---------- generations ---------- */

static gen *load_gen(const char *dir, int factory) {
    char *plan = andysu_load_plan(dir, O.abi);
    if (jerr(plan)) {
        report("load_failed", plan);
        return NULL;
    }
    gen *g = calloc(1, sizeof *g);
    snprintf(g->dir, sizeof g->dir, "%s", dir);
    g->factory = factory;
    char *v = jstr(plan, "version"), *entry = jstr(plan, "entry_lib"), *sym = jstr(plan, "symbol");
    snprintf(g->version, sizeof g->version, "%s", v ? v : "?");
    void *entry_h = NULL;
    const char *p = jfind(plan, "libs");
    if (p && *p == '[') p++;
    while (p && *p == '"' && g->nh < MAX_LIBS) {
        char *lib = jparse_str(p, &p);
        void *h = dlopen(lib, RTLD_NOW | RTLD_LOCAL);
        if (!h) {
            char *e = jesc(dlerror());
            out("{\"event\":\"load_failed\",\"lib\":\"%s\",\"error\":\"%s\"}", lib, e);
            free(e);
            free(lib);
            free(g);
            g = NULL;
            goto done;
        }
        g->handles[g->nh++] = h;
        if (entry && strcmp(lib, entry) == 0) entry_h = h;
        free(lib);
        if (*p == ',') p++;
    }
    const andysu_payload *(*fn)(void) = entry_h && sym ? (const andysu_payload *(*)(void))dlsym(entry_h, sym) : NULL;
    if (!fn || !(g->p = fn()) || g->p->struct_size < sizeof(andysu_payload) - sizeof(void *) || !g->p->start) {
        out("{\"event\":\"load_failed\",\"dir\":\"%s\",\"error\":\"entry symbol missing or invalid\"}", dir);
        free(g);
        g = NULL;
    }
done:
    free(v);
    free(entry);
    free(sym);
    andysu_free(plan);
    return g;
}

static int start_gen(gen *g, const char *state, size_t len) {
    H.slot_dir = g->dir;
    H.version = g->version;
    int rc = g->p->start(&H, state, len);
    out("{\"event\":\"started\",\"version\":\"%s\",\"factory\":%s,\"rc\":%d,\"trial\":%s}", g->version,
        g->factory ? "true" : "false", rc, in_trial ? "true" : "false");
    return rc;
}

static char *config_json(void) {
    char *pem = jesc(g_pub_pem);
    size_t n = strlen(pem) + strlen(O.app) + strlen(O.channel) + strlen(O.abi) + 256;
    char *c = malloc(n);
    snprintf(c, n,
             "{\"app\":\"%s\",\"channel\":\"%s\",\"roots\":[\"%s\"],\"device\":{\"host_api\":%d,\"abis\":[\"%s\"]}}",
             O.app, O.channel, pem, ANDYSU_HOST_API_LEVEL, O.abi);
    free(pem);
    return c;
}

/* Back to the previous generation, inside this process, with the state from before the swap. */
static void live_rollback(const char *reason) {
    char *esc = jesc(reason);
    out("{\"event\":\"live_rollback\",\"reason\":\"%s\",\"from\":\"%s\",\"to\":\"%s\"}", esc, cur->version,
        prev ? prev->version : "none");
    free(esc);
    if (cur->p->suspend) {
        size_t l = 0;
        free(cur->p->suspend(&l)); /* release the broken generation's resources */
    }
    report("rolled_back", andysu_report_failure(O.root, reason));
    in_trial = 0;
    cur = prev;
    prev = NULL;
    start_gen(cur, swap_state, swap_state_len);
    free(swap_state);
    swap_state = NULL;
}

static void apply_live(void) {
    size_t len = 0;
    char *st = cur->p->suspend ? cur->p->suspend(&len) : NULL;
    out("{\"event\":\"suspended\",\"version\":\"%s\",\"state\":%s}", cur->version, st && len ? st : "null");
    char *r = andysu_begin_live(O.root, now_s());
    char *dir = jstr(r, "slot_dir");
    if (jerr(r) || !dir) {
        report("begin_live_failed", r);
        start_gen(cur, st, len);
        free(st);
        free(dir);
        return;
    }
    andysu_free(r);
    gen *ng = load_gen(dir, 0);
    free(dir);
    swap_state = st;
    swap_state_len = len;
    prev = cur;
    if (!ng) {
        /* nothing of the new generation ran: roll back without suspending it */
        report("rolled_back", andysu_report_failure(O.root, "new generation failed to load"));
        cur = prev;
        prev = NULL;
        start_gen(cur, swap_state, swap_state_len);
        free(swap_state);
        swap_state = NULL;
        return;
    }
    cur = ng;
    in_trial = 1;
    trial_live = 1;
    trial_deadline = now_ms() + O.watchdog_ms;
    live_swaps++;
    out("{\"event\":\"live_swapped\",\"from\":\"%s\",\"to\":\"%s\",\"live_swaps\":%d}", prev->version, cur->version,
        live_swaps);
    int rc = start_gen(cur, st, len);
    if (rc != 0) {
        char why[64];
        snprintf(why, sizeof why, "start() returned %d", rc);
        live_rollback(why);
    }
}

static void do_check(void) {
    char *cfg = config_json();
    char *r = andysu_update_http(O.root, cfg, O.url, r16, now_s());
    free(cfg);
    int staged = !jerr(r) && jfind(r, "staged") && *jfind(r, "staged") == '"';
    report("check", r);
    if (!staged) {
        /* also apply something staged earlier (e.g. by a check before a restart) */
        char *s = andysu_state(O.root);
        const char *p = jfind(s, "pending");
        staged = p && *p == '{';
        andysu_free(s);
    }
    if (staged && O.live && !in_trial) apply_live();
    else if (staged && O.apply && !strcmp(O.apply, "restart") && !in_trial) {
        /* Apply by restarting: suspend cleanly, then re-execute; the launch promotes the slot. */
        size_t len = 0;
        free(cur->p->suspend ? cur->p->suspend(&len) : NULL);
        relaunch("applying update");
    }
}

/* ---------- main ---------- */

static char *slurp(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *b = malloc((size_t)n + 1);
    size_t k = fread(b, 1, (size_t)n, f);
    b[k] = 0;
    fclose(f);
    return b;
}

static void usage(void) {
    fprintf(stderr,
            "usage: andysu-desktop-host --root DIR --factory DIR --url MANIFEST_URL --pub KEY.pem --app ID\n"
            "       [--channel stable] [--abi x86_64] [--apply live|restart|next_start] [--live]\n"
            "       [--watchdog-ms 5000] [--check-ms 0]\n"
            "       [--max-attempts 2]\n");
    exit(2);
}

int main(int argc, char **argv) {
    g_argv = argv;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        const char *v = i + 1 < argc ? argv[i + 1] : NULL;
#define OPT(name, field) else if (!strcmp(a, name) && v) { O.field = v; i++; }
        if (!strcmp(a, "--live")) O.live = 1;
        else if (!strcmp(a, "--apply") && v) {
            O.apply = v;
            O.live = !strcmp(v, "live");
            i++;
        }
        OPT("--root", root) OPT("--url", url) OPT("--pub", pub_path) OPT("--app", app) OPT("--channel", channel)
        OPT("--abi", abi) OPT("--factory", factory)
        else if (!strcmp(a, "--watchdog-ms") && v) { O.watchdog_ms = atol(v); i++; }
        else if (!strcmp(a, "--check-ms") && v) { O.check_ms = atol(v); i++; }
        else if (!strcmp(a, "--max-attempts") && v) { O.max_attempts = atoi(v); i++; }
        else usage();
#undef OPT
    }
    if (!O.root || !O.url || !O.pub_path || !O.app || !O.factory) usage();
    if (!(g_pub_pem = slurp(O.pub_path))) { perror(O.pub_path); return 2; }
    ssize_t n = readlink("/proc/self/exe", g_self, sizeof g_self - 1);
    if (n <= 0) { perror("readlink"); return 2; }
    g_self[n] = 0;
    int fd = open("/dev/urandom", O_RDONLY);
    if (fd < 0 || read(fd, r16, 16) != 16) { perror("urandom"); return 2; }
    close(fd);

    H.api_level = ANDYSU_HOST_API_LEVEL;
    H.struct_size = sizeof H;
    H.log = h_log;
    H.mark_healthy = h_mark_healthy;
    H.emit = h_emit;
    H.request_update_check = h_request_check;

    char *mp = andysu_crash_marker_path(O.root);
    char *m = jstr(mp, "ok");
    andysu_free(mp);
    if (!m) { fprintf(stderr, "bad root %s\n", O.root); return 2; }
    snprintf(g_marker, sizeof g_marker, "%s", m);
    free(m);
    const char *rl = getenv("ANDYSU_RELAUNCHES");
    g_relaunches = rl ? atoi(rl) : 0;
    snprintf(g_env_relaunch, sizeof g_env_relaunch, "ANDYSU_RELAUNCHES=%d", g_relaunches + 1);
    install_crash_handlers();
    out("{\"event\":\"boot\",\"pid\":%d,\"relaunches\":%s,\"live\":%s,\"core\":\"%s\"}", (int)getpid(), rl ? rl : "0",
        O.live ? "true" : "false", andysu_version());

    /* Cold-start decision, persisted by the core before we load anything. */
    char *d = andysu_launch(O.root, r16, now_s(), true, (uint32_t)O.max_attempts);
    char *dir = jstr(d, "slot_dir");
    in_trial = jbool(d, "in_trial");
    trial_live = 0;
    if (report("launch", d) != 0) return 4;
    cur = dir ? load_gen(dir, 0) : NULL;
    if (dir && !cur) {
        report("rolled_back", andysu_report_failure(O.root, "slot failed to load"));
        relaunch("slot failed to load");
    }
    free(dir);
    if (!cur && !(cur = load_gen(O.factory, 1))) {
        fprintf(stderr, "factory payload failed to load\n");
        return 5;
    }
    if (in_trial) trial_deadline = now_ms() + O.watchdog_ms;
    int rc = start_gen(cur, NULL, 0);
    if (rc != 0) {
        if (in_trial) {
            report("rolled_back", andysu_report_failure(O.root, "start() failed on cold boot"));
            relaunch("start failed");
        }
        return 6;
    }
    /* We got far enough: a later crash may relaunch again. */
    unsetenv("ANDYSU_RELAUNCHES");
    g_relaunches = 0;
    snprintf(g_env_relaunch, sizeof g_env_relaunch, "ANDYSU_RELAUNCHES=1");

    long next_check = O.check_ms > 0 ? now_ms() + O.check_ms : -1;
    char line[256];
    size_t ll = 0;
    for (;;) {
        fd_set rs;
        FD_ZERO(&rs);
        FD_SET(0, &rs);
        struct timeval tv = {0, 16000};
        if (select(1, &rs, NULL, NULL, &tv) > 0) {
            char c;
            ssize_t k = read(0, &c, 1);
            if (k <= 0) break; /* stdin closed: quit */
            if (c != '\n' && ll + 1 < sizeof line) { line[ll++] = c; continue; }
            line[ll] = 0;
            ll = 0;
            if (!strcmp(line, "check")) check_requested = 1;
            else if (!strcmp(line, "quit")) break;
            else if (!strcmp(line, "state")) report("state", andysu_state(O.root));
        }
        if (cur->p->frame && cur->p->frame()) break;
        if (in_trial && now_ms() > trial_deadline) {
            out("{\"event\":\"watchdog\",\"version\":\"%s\"}", cur->version);
            if (trial_live && prev) live_rollback("watchdog: markHealthy not called in time");
            else {
                report("rolled_back", andysu_report_failure(O.root, "watchdog: markHealthy not called in time"));
                relaunch("watchdog");
            }
        }
        if (next_check > 0 && now_ms() > next_check) {
            check_requested = 1;
            next_check = now_ms() + O.check_ms;
        }
        if (check_requested && !in_trial) {
            check_requested = 0;
            do_check();
        }
    }
    size_t len = 0;
    char *st = cur->p->suspend ? cur->p->suspend(&len) : NULL;
    out("{\"event\":\"exit\",\"version\":\"%s\",\"state\":%s}", cur->version, st && len ? st : "null");
    free(st);
    return 0;
}
