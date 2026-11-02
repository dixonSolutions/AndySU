/* A stand-in for the engine: one mutable global and an identity string. */
#include <stdint.h>
static volatile int counter = 0;
__attribute__((visibility("default"))) volatile int *counter_addr(void) { return &counter; }
__attribute__((visibility("default"))) int *id_addr(void) { static int id; return &id; }
