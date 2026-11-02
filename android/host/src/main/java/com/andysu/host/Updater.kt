package com.andysu.host

import android.content.Context
import android.net.ConnectivityManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.storage.StorageManager
import android.util.Log
import com.andysu.api.UpdateClient
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors

/**
 * The host's updater (also the recovery updater): HTTP transport around the core's sans-IO
 * session. Conditional manifest fetch (ETag), signature and policy checks in the core, resumable
 * downloads (Range), deltas with fallback to full blobs, staging, then apply per policy.
 */
class Updater(private val ctx: Context, private val cfg: Config, private val root: File) : UpdateClient {
    private val exec = Executors.newSingleThreadExecutor { r -> Thread(r, "andysu-updater") }
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var status = JSONObject().put("state", "idle")
    @Volatile private var listener: UpdateClient.Listener? = null
    @Volatile internal var recoveryStatus: ((String) -> Unit)? = null

    override fun checkNow() = checkAsync("user")
    override fun status(): String = status.toString()
    override fun setListener(listener: UpdateClient.Listener?) { this.listener = listener }
    override fun setApplyPolicy(policy: String?) = AndySU.setApplyPolicy(policy)
    override fun applyPolicy(): String = AndySU.applyPolicy()
    override fun applyNow() = main.post {
        if (status.optString("state") == "shell_update") exec.execute { ShellUpdate.install(ctx, status) }
        else if (!AndySU.applyLive()) restartNow()
    }.let { }
    override fun restartNow() = Phoenix.relaunch(ctx, "restart requested")

    fun checkAsync(trigger: String) = exec.execute { runCheck(trigger) }
    fun gcAsync() = exec.execute {
        try {
            AndySU.emit("gc", JSONObject().put("result", Core.obj(Core.gc(root.path, lowStorage()))))
        } catch (e: Exception) {
            Log.w(AndySU.TAG, "gc failed", e)
        }
    }

    private fun setStatus(state: String, extra: JSONObject? = null) {
        val s = JSONObject(extra?.toString() ?: "{}").put("state", state)
        status = s
        AndySU.emit("update_status", JSONObject().put("status", s))
        listener?.let { l -> main.post { l.onStatus(s.toString()) } }
        recoveryStatus?.invoke(state)
    }

    private fun networkAllowed(): Boolean {
        // A loopback remote (development, adb reverse) needs no network.
        val host = runCatching { URL(cfg.manifestUrl).host }.getOrDefault("")
        if (host == "127.0.0.1" || host == "localhost") return true
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        @Suppress("DEPRECATION")
        val active = cm.activeNetworkInfo
        if (active == null || !active.isConnected) return false
        return cfg.network != "unmetered" || !cm.isActiveNetworkMetered
    }

    private fun allocatable(): Long = try {
        if (Build.VERSION.SDK_INT >= 26) {
            val sm = ctx.getSystemService(StorageManager::class.java)
            sm.getAllocatableBytes(sm.getUuidForPath(root))
        } else root.usableSpace
    } catch (e: Exception) {
        root.usableSpace
    }

    private fun lowStorage() = allocatable() < 200L * 1024 * 1024

    /** Blocking; runs on the updater thread or a job thread. */
    @Synchronized
    fun runCheck(trigger: String) {
        if (cfg.mode == "off") return
        if (trigger != "job" && !networkAllowed()) {
            setStatus("deferred", JSONObject().put("reason", "network policy ${cfg.network}"))
            return
        }
        setStatus("checking", JSONObject().put("trigger", trigger))
        val h = (Core.ok(Core.sessionNew(root.path, cfg.coreConfig(root), AndySU.random16())) as Number).toLong()
        try {
            val etag = Core.ok(Core.etag(root.path)) as String?
            val cached = File(root, "manifest.json").exists()
            val m = Http.get(cfg.manifestUrl, if (cached) etag else null)
            val outcome = if (m.code == 304) Core.obj(Core.sessionCheckCached(h, AndySU.nowS()))
            else {
                val sig = Http.get(cfg.manifestUrl + ".sig", null)
                Core.obj(Core.sessionCheck(h, cfg.manifestUrl, m.body, sig.body, m.etag, AndySU.nowS()))
            }
            outcome.put("not_modified", m.code == 304)
            when (outcome.getString("outcome")) {
                "up_to_date" -> setStatus("up_to_date", outcome)
                "shell_update" -> setStatus("shell_update", outcome)
                "update" -> {
                    if (cfg.mode == "check_only") return setStatus("available", outcome)
                    if (!outcome.optBoolean("already_staged")) {
                        val need = outcome.optLong("download_bytes")
                        if (need * 2 > allocatable()) {
                            Core.gc(root.path, true)
                            if (need * 2 > allocatable()) return setStatus("error", JSONObject().put("code", "no_space").put("needed", need))
                        }
                        setStatus("downloading", outcome)
                        download(h)
                        Core.ok(Core.sessionStage(h))
                    }
                    setStatus("staged", outcome)
                    if (trigger == "recovery") Phoenix.relaunch(ctx, "recovery update staged")
                    if (cfg.mode == "auto") main.post { AndySU.onStaged(outcome) }
                }
            }
        } catch (e: CoreException) {
            Log.w(AndySU.TAG, "update check rejected: ${e.code}: ${e.message}")
            setStatus("error", JSONObject().put("code", e.code).put("message", e.message))
        } catch (e: IOException) {
            Log.w(AndySU.TAG, "update check failed: $e")
            setStatus("error", JSONObject().put("code", "net").put("message", e.toString()))
        } finally {
            Core.sessionFree(h)
        }
    }

    private fun download(h: Long) {
        repeat(4) {
            val plan = Core.arr(Core.sessionPlan(h))
            if (plan.length() == 0) return
            for (i in 0 until plan.length()) {
                val f = plan.getJSONObject(i)
                if (!fetchOne(h, f)) {
                    if (f.getString("kind") == "delta") Core.ok(Core.sessionSkipDelta(h, f.getString("key")))
                    else throw IOException("could not download ${f.getString("key")}")
                }
            }
        }
    }

    private fun fetchOne(h: Long, f: JSONObject): Boolean {
        val key = f.getString("key")
        val dest = File(f.getString("dest"))
        val size = f.getLong("size")
        val urls = f.getJSONArray("urls")
        for (u in 0 until urls.length()) {
            for (attempt in 0 until 3) {
                try {
                    val offset = if (dest.exists()) minOf(dest.length(), size) else 0L
                    if (offset > 0) AndySU.emit("resume", JSONObject().put("key", key).put("offset", offset))
                    Http.download(urls.getString(u), dest, offset, size)
                    Core.ok(Core.sessionComplete(h, key))
                    return true
                } catch (e: CoreException) {
                    Log.w(AndySU.TAG, "verification failed for $key: ${e.message}")
                    break // wrong bytes from this URL: next URL (or, for a delta, the full blob)
                } catch (e: IOException) {
                    Log.w(AndySU.TAG, "download of $key interrupted: $e")
                    AndySU.emit("download_interrupted", JSONObject().put("key", key).put("error", e.toString()))
                    Thread.sleep(500L shl attempt)
                }
            }
        }
        return false
    }
}

internal object Http {
    class Res(val code: Int, val body: ByteArray, val etag: String?)

    private fun open(url: String): HttpURLConnection = (URL(url).openConnection() as HttpURLConnection).apply {
        connectTimeout = 15_000
        readTimeout = 30_000
        instanceFollowRedirects = true
        setRequestProperty("User-Agent", "AndySU/${Core.versionString()}")
    }

    fun get(url: String, etag: String?): Res {
        val c = open(url)
        if (etag != null) c.setRequestProperty("If-None-Match", etag)
        try {
            val code = c.responseCode
            if (code == 304) return Res(304, ByteArray(0), etag)
            if (code !in 200..299) throw IOException("HTTP $code for $url")
            val body = c.inputStream.use { it.readBytes() }
            return Res(code, body, c.getHeaderField("ETag"))
        } finally {
            c.disconnect()
        }
    }

    /** Append `url` to `dest` from `offset` (Range); restart if the server ignores the range. */
    fun download(url: String, dest: File, offset: Long, size: Long) {
        val c = open(url)
        if (offset > 0) c.setRequestProperty("Range", "bytes=$offset-")
        try {
            val code = c.responseCode
            if (code !in 200..299) throw IOException("HTTP $code for $url")
            val append = offset > 0 && code == 206
            FileOutputStream(dest, append).use { out ->
                c.inputStream.use { inp ->
                    val buf = ByteArray(64 * 1024)
                    var total = if (append) offset else 0L
                    while (true) {
                        val n = inp.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        total += n
                        if (total > size) throw IOException("more bytes than expected")
                    }
                    out.flush()
                    out.fd.sync()
                    if (total < size) throw IOException("connection closed at $total of $size bytes")
                }
            }
        } finally {
            c.disconnect()
        }
    }
}
