#include <stdlib.h>
#include <andysu_payload.h>

static const andysu_host *host;
static int start(const andysu_host *h, const char *state, size_t len) {
    (void)state; (void)len;
    host = h;
    h->log("hello from a C payload");
    h->mark_healthy();
    return 0;
}
static char *suspend(size_t *len) { *len = 0; return NULL; }
ANDYSU_PAYLOAD(start, suspend)
