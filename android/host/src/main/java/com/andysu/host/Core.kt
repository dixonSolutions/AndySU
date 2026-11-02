package com.andysu.host

import org.json.JSONArray
import org.json.JSONObject

class CoreException(val code: String, message: String) : Exception(message)

/** JNI binding of the AndySU core (libandysu_core.so). Every call returns the core's JSON envelope. */
object Core {
    init {
        System.loadLibrary("andysu_core")
    }

    @JvmStatic external fun version(): String

    /** The core version as plain text. */
    fun versionString(): String = ok(version()) as String
    @JvmStatic external fun sessionNew(root: String, config: String, random16: ByteArray): String
    @JvmStatic external fun sessionFree(h: Long): String
    @JvmStatic external fun sessionCheck(h: Long, url: String, manifest: ByteArray, sig: ByteArray, etag: String?, now: Long): String
    @JvmStatic external fun sessionCheckCached(h: Long, now: Long): String
    @JvmStatic external fun sessionPlan(h: Long): String
    @JvmStatic external fun sessionComplete(h: Long, key: String): String
    @JvmStatic external fun sessionSkipDelta(h: Long, key: String): String
    @JvmStatic external fun sessionStage(h: Long): String
    @JvmStatic external fun etag(root: String): String
    @JvmStatic external fun launch(root: String, random16: ByteArray, now: Long, applyPending: Boolean, maxAttempts: Int): String
    @JvmStatic external fun markHealthy(root: String): String
    @JvmStatic external fun reportFailure(root: String, reason: String): String
    @JvmStatic external fun beginLive(root: String, now: Long): String
    @JvmStatic external fun reset(root: String, reason: String): String
    @JvmStatic external fun gc(root: String, lowStorage: Boolean): String
    @JvmStatic external fun state(root: String): String
    @JvmStatic external fun crashMarkerPath(root: String): String
    @JvmStatic external fun loadPlan(dir: String, abi: String): String
    @JvmStatic external fun verifyFile(path: String, sha256: String, size: Long): String

    /** Unwraps `{"ok": v}`; throws [CoreException] for `{"error": ...}`. */
    fun ok(json: String): Any? {
        val o = JSONObject(json)
        if (o.has("error")) {
            val e = o.getJSONObject("error")
            throw CoreException(e.optString("code"), e.optString("message"))
        }
        val v = o.opt("ok")
        return if (v == JSONObject.NULL) null else v
    }

    fun obj(json: String): JSONObject = ok(json) as? JSONObject ?: JSONObject()
    fun arr(json: String): JSONArray = ok(json) as? JSONArray ?: JSONArray()
}

internal fun JSONObject.str(key: String): String? = if (isNull(key)) null else optString(key)
internal fun JSONObject.objOrNull(key: String): JSONObject? = if (isNull(key)) null else optJSONObject(key)
