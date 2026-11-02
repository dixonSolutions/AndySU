#include <android/dlext.h>
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
typedef volatile int *(*addr_fn)(void);
static int check(const char *label, void *h1, void *h2) {
    if (!h1 || !h2) { printf("[%s] load failed: h1=%p h2=%p (%s)\n", label, h1, h2, dlerror()); return 1; }
    addr_fn f1 = (addr_fn)dlsym(h1, "counter_addr"), f2 = (addr_fn)dlsym(h2, "counter_addr");
    volatile int *a = f1(), *b = f2();
    *a = 1; *b = 2;
    int separate = (a != b) && (*a == 1) && (*b == 2);
    printf("[%s] handles %s; counter addresses %p vs %p -> globals %s\n", label,
           h1 == h2 ? "SAME" : "distinct", (void *)a, (void *)b, separate ? "SEPARATE" : "SHARED");
    *a = 0; *b = 0;
    return 0;
}
int main(int argc, char **argv) {
    const char *dir = argv[1];
    char p1[256], p2[256], p3[256], p4[256];
    snprintf(p1, sizeof p1, "%s/libcounter_a.so", dir);   /* original, SONAME libcounter.so */
    snprintf(p2, sizeof p2, "%s/libcounter_b.so", dir);   /* byte-identical copy, other file name, same SONAME */
    snprintf(p3, sizeof p3, "%s/libcounter_link.so", dir);/* symlink to a */
    snprintf(p4, sizeof p4, "%s/libcounter2.so", dir);    /* other bytes AND other SONAME (what AndySU's pack produces) */
    void *a = dlopen(p1, RTLD_NOW | RTLD_LOCAL);
    printf("opened a: %p\n", a);
    check("T1 identical bytes, different file name, same SONAME", a, dlopen(p2, RTLD_NOW | RTLD_LOCAL));
    check("T2 symlink to a (control: expect SAME)", a, dlopen(p3, RTLD_NOW | RTLD_LOCAL));
    check("T3 same path again (control: expect SAME)", a, dlopen(p1, RTLD_NOW | RTLD_LOCAL));
    android_dlextinfo ext; memset(&ext, 0, sizeof ext);
    ext.flags = ANDROID_DLEXT_FORCE_LOAD;
    check("T4 same path, android_dlopen_ext FORCE_LOAD", a, android_dlopen_ext(p1, RTLD_NOW | RTLD_LOCAL, &ext));
    void *f1 = android_dlopen_ext(p1, RTLD_NOW | RTLD_LOCAL, &ext), *f2 = android_dlopen_ext(p1, RTLD_NOW | RTLD_LOCAL, &ext);
    check("T5 FORCE_LOAD twice", f1, f2);
    check("T6 different SONAME + file name", a, dlopen(p4, RTLD_NOW | RTLD_LOCAL));
    return 0;
}
