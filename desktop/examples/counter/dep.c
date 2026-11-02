/* A dependency of the counter payload. Each loaded copy counts itself, so a generation can prove
 * it got a fresh copy of its own dependency rather than the previous generation's. */
static int instances;
__attribute__((constructor)) static void loaded(void) { instances++; }
int dep_instances(void) { return instances; }
int dep_version(void) { return DEP_VERSION; }
