package com.andysu.host

import android.app.Activity
import android.util.Log

/** JNI binding of libandysu_host.so: native payload generations and the crash handler. */
object NativeBridge {
    init {
        System.loadLibrary("andysu_host")
    }

    @JvmStatic external fun load(libs: Array<String>, entryLib: String, symbol: String, slotDir: String, version: String): Long
    @JvmStatic external fun lastError(): String
    @JvmStatic external fun start(handle: Long, state: ByteArray?, activity: Activity?, width: Int, height: Int): Int
    @JvmStatic external fun frame(handle: Long, width: Int, height: Int): Int
    @JvmStatic external fun suspend(handle: Long): ByteArray?
    @JvmStatic external fun installCrashHandler(markerPath: String, notifyFd: Int)
    /**
     * Checks whether a payload replaced the host's signal handlers and emits crash_handler_replaced.
     * With [takeover] the host's handlers are re-installed (the payload's chained); otherwise nothing changes.
     * Returns the number of signals reported.
     */
    @JvmStatic external fun checkCrashHandlers(takeover: Boolean): Int
    @JvmStatic external fun setRelaunchOnCrash(on: Boolean)
    @JvmStatic external fun crashForTest()

    // Called from native payloads through the andysu_host callbacks (any thread).
    // Block bodies: the JNI side looks these up with a void return type.
    @JvmStatic fun onMarkHealthy() { AndySU.markHealthy() }
    @JvmStatic fun onLog(msg: String) { Log.i(AndySU.TAG, "payload: $msg") }
    @JvmStatic fun onEmit(json: String) { AndySU.emitRaw(json) }
    @JvmStatic fun onRequestCheck() { AndySU.updater.checkAsync("payload") }
}
