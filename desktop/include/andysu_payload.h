/*
 * AndySU native payload contract, Host API level 1.
 *
 * A payload is a shared library exporting `andysu_payload_v1`, usually through
 *
 *     ANDYSU_PAYLOAD(my_start, my_suspend)
 *
 * The host calls start() once per generation, with the state the previous generation returned
 * from suspend() (or NULL/0 on a cold start). suspend() must release threads, GL objects, audio
 * and file handles and return versioned, language-neutral state (JSON recommended) allocated with
 * malloc(); the host frees it with free(). After a successful start the payload calls
 * host->mark_healthy() once it is sure it works; until then it is on TRIAL and the host rolls back
 * on an init failure, a crash, or a watchdog timeout.
 *
 * Host API levels are additive: a level-N host offers every field of levels 1..N. Read
 * host->api_level before touching fields added after level 1.
 */
#ifndef ANDYSU_PAYLOAD_H
#define ANDYSU_PAYLOAD_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ANDYSU_HOST_API_LEVEL 1

typedef struct andysu_host {
    uint32_t api_level;        /* >= 1 */
    uint32_t struct_size;      /* sizeof(andysu_host) of the host that built it */
    const char *slot_dir;      /* absolute path of this generation's slot (read-only code) */
    const char *version;       /* payload version from andysu-payload.json */
    void (*log)(const char *msg);
    /* Commit this generation. Call once, after the payload is known to work. */
    void (*mark_healthy)(void);
    /* Structured event for UIs and tests (one JSON object, no newline). */
    void (*emit)(const char *json);
    /* Ask the host to check for updates soon (non-blocking). */
    void (*request_update_check)(void);
    void *host_data;
    /* Android only (NULL/0 elsewhere). frame() runs on the host's GL thread with the host's EGL
     * context current; the host keeps the context and surface alive across live swaps. */
    void *java_vm;             /* JavaVM* */
    void *activity;            /* jobject: global ref to the current stub Activity, may be NULL */
    int32_t surface_width;
    int32_t surface_height;
} andysu_host;

typedef struct andysu_payload {
    uint32_t struct_size;
    /* 0 on success. Non-zero means init failed: the host rolls back. */
    int (*start)(const andysu_host *host, const char *state, size_t state_len);
    /* Release everything; return malloc'd state (may be NULL). */
    char *(*suspend)(size_t *state_len);
    /* Optional per-frame tick on the host's thread; non-zero asks the host to quit. */
    int (*frame)(void);
} andysu_payload;

#if defined(_WIN32)
#define ANDYSU_EXPORT __declspec(dllexport)
#else
#define ANDYSU_EXPORT __attribute__((visibility("default")))
#endif

#define ANDYSU_PAYLOAD_FULL(start_fn, suspend_fn, frame_fn)                                  \
    ANDYSU_EXPORT const andysu_payload *andysu_payload_v1(void) {                          \
        static const andysu_payload p = {sizeof(andysu_payload), start_fn, suspend_fn, frame_fn}; \
        return &p;                                                                        \
    }

#define ANDYSU_PAYLOAD(start_fn, suspend_fn) ANDYSU_PAYLOAD_FULL(start_fn, suspend_fn, NULL)

#ifdef __cplusplus
}
#endif
#endif
