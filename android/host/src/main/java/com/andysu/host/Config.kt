package com.andysu.host

import android.content.Context
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject

/**
 * Frozen host configuration, read from `assets/andysu/config.json` (generated from andysu.toml
 * when the host APK is built).
 */
class Config(val json: JSONObject) {
    val app: String = json.getString("app")
    val channel: String = json.optString("channel", "stable")
    val roots: List<String> = json.getJSONArray("roots").let { a -> (0 until a.length()).map { a.getString(it) } }
    val manifestUrl: String = json.getString("manifest_url")
    /** auto | download_only | check_only | off */
    val mode: String = json.optString("mode", "auto")
    /** How staged updates apply: live (in-process swap, default) | restart | next_start */
    val apply: String = json.optString("apply", "live")
    /** For apply = restart: now (one relaunch right away) | background (when the app is next in the background) */
    val restartWhen: String = json.optString("restart_when", "now")
    /** What apply = live does when a live swap is impossible (transparent mode): restart | next_start */
    val liveFallback: String = json.optString("live_fallback", "next_start")
    /** any | unmetered */
    val network: String = json.optString("network", "any")
    val charging: Boolean = json.optBoolean("charging", false)
    val batteryNotLow: Boolean = json.optBoolean("battery_not_low", true)
    val checkOnOpen: Boolean = json.optBoolean("check_on_open", true)
    val checkIntervalMinutes: Long = json.optLong("check_interval_minutes", 360)
    val watchdogMs: Long = json.optLong("watchdog_ms", 10_000)
    val maxBootAttempts: Int = json.optInt("max_boot_attempts", 2)
    /**
     * Host API level used for release selection: what andysu.toml declares (the level `pack`
     * checks payloads against), but never more than this AAR implements.
     */
    val hostApi: Int = json.optInt("host_api", 1).let { declared ->
        if (declared > HostApiImpl.LEVEL) {
            android.util.Log.w("AndySU", "config declares host_api $declared; this host implements ${HostApiImpl.LEVEL}")
        }
        minOf(declared, HostApiImpl.LEVEL)
    }
    val shellVersion: Long = json.optLong("shell_version", 0)
    val stubActivities: Int = json.optInt("stub_activities", 4)
    val stubServices: Int = json.optInt("stub_services", 2)

    /** Config object for the core session. The built-in payload (factory dirs under `root`)
     *  serves as a delta base and source of unchanged bundles. */
    fun coreConfig(root: java.io.File): String = JSONObject()
        .put("base_dirs", JSONArray(java.io.File(root, "factory").listFiles()
            ?.filter { java.io.File(it, "andysu-slot.json").isFile }?.map { it.path } ?: emptyList<String>()))
        .put("app", app)
        .put("channel", channel)
        .put("roots", JSONArray(roots))
        .put("device", JSONObject()
            .put("host_api", hostApi)
            .put("shell_version", shellVersion)
            .put("api_level", Build.VERSION.SDK_INT)
            .put("abis", JSONArray(Build.SUPPORTED_ABIS.toList())))
        .toString()

    companion object {
        fun load(ctx: Context): Config =
            Config(JSONObject(ctx.assets.open("andysu/config.json").use { it.readBytes().decodeToString() }))
    }
}
