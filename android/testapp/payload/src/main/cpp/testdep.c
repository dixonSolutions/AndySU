/* Dependency of libtestnative. Counts its own loaded copies, so each payload generation can
 * prove it got a fresh copy (renamed DT_NEEDED) rather than the previous generation's. */
static int instances;
__attribute__((constructor)) static void loaded(void) { instances++; }
int dep_instances(void) { return instances; }
int dep_version(void) { return TP_VERSION; }
