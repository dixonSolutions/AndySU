#include <jni.h>
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
