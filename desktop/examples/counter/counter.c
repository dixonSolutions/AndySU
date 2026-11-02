/*
 * Sample native payload: counts frames, carries the count across generations as JSON state,
 * calls mark_healthy() after 10 frames. Build-time BEHAVIOR selects failure modes for tests:
 *   0 ok, 1 start() fails, 2 never healthy, 3 crash in a frame during trial, 4 crash in start().
 */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "andysu_payload.h"

#ifndef VERSION
#define VERSION "dev"
#endif
#ifndef BEHAVIOR
#define BEHAVIOR 0
#endif

int dep_instances(void);
int dep_version(void);

static const andysu_host *host;
static long counter, frames;
static int healthy;

static int start(const andysu_host *h, const char *state, size_t len) {
    host = h;
    frames = 0;
    healthy = 0;
    if (state && len) {
        const char *p = strstr(state, "\"counter\":");
        if (p) counter = atol(p + 10);
    }
#if BEHAVIOR == 1
    return 7;
#elif BEHAVIOR == 4
    raise(SIGSEGV);
#endif
    char ev[256];
    snprintf(ev, sizeof ev,
             "{\"event\":\"payload\",\"version\":\"%s\",\"counter\":%ld,\"dep_version\":%d,\"dep_instances\":%d}",
             VERSION, counter, dep_version(), dep_instances());
    h->emit(ev);
    return 0;
}

static char *suspend(size_t *len) {
    char *s = malloc(64);
    *len = (size_t)snprintf(s, 64, "{\"v\":1,\"counter\":%ld}", counter);
    return s;
}

static int frame(void) {
    counter++;
    frames++;
#if BEHAVIOR == 3
    if (frames == 5) {
        volatile int *null = NULL;
        *null = 1;
    }
#endif
#if BEHAVIOR != 2
    if (frames == 10 && !healthy) {
        healthy = 1;
        host->mark_healthy();
    }
#endif
    return 0;
}

ANDYSU_PAYLOAD_FULL(start, suspend, frame)
