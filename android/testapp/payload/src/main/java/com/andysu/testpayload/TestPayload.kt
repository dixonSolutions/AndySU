package com.andysu.testpayload

import android.app.Activity
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.util.Log
import android.widget.TextView
import com.andysu.api.HostApi
import com.andysu.api.PayloadAdapter
import dalvik.system.DexClassLoader
import org.json.JSONObject
import java.io.File
import kotlin.concurrent.thread

object Native {
    init { System.loadLibrary("testnative") }
    @JvmStatic external fun depVersion(): Int
    @JvmStatic external fun depInstances(): Int
    @JvmStatic external fun pageSize(): Int
    @JvmStatic external fun crash()
    @JvmStatic external fun installOwnHandler()
    @JvmStatic external fun installHandlingHandler()
    @JvmStatic external fun faultHandled(): Int
}

/** Test payload: emits JSON events on logcat tag AndySU-Event; behaviour from BuildConfig. */
class TestPayload : PayloadAdapter() {
    private lateinit var host: HostApi
    private val main = Handler(Looper.getMainLooper())
    private var counter = 0L
    private var running = false
    private var view: TextView? = null
    private val behavior = BuildConfig.TP_BEHAVIOR

    private fun emit(event: String, o: JSONObject = JSONObject()) {
        o.put("event", event).put("version", BuildConfig.TP_VERSION).put("behavior", behavior).put("pid", Process.myPid())
        // liblog drops writes when logd is busy; the harness falls back to this file (see Device.wait).
        // The drop-log marker (harness only) makes the next emit of that event skip logcat, to test that.
        val dropped = recordEvent(event, o)
        if (!dropped) Log.i("AndySU-Event", o.toString())
    }

    /** Remembers the latest emit of each event in filesDir; returns true when the harness asked to drop this log line. */
    @Synchronized
    private fun recordEvent(event: String, o: JSONObject): Boolean {
        var drop = false
        try {
            val dir = host.context().filesDir
            val marker = File(dir, "drop-log")
            if (marker.isFile && marker.readText().trim() == event) {
                marker.delete()
                drop = true
            }
            val file = File(dir, "andysu-last-events.json")
            val all = try { JSONObject(file.readText()) } catch (e: Exception) { JSONObject() }
            all.put(event, o)
            val tmp = File(dir, "andysu-last-events.json.tmp")
            tmp.writeText(all.toString())
            tmp.renameTo(file)
        } catch (e: Exception) {
            // diagnostics only: never let it affect the payload
        }
        return drop
    }

    private val tick = object : Runnable {
        override fun run() {
            if (!running) return
            counter++
            view?.text = "AndySU test payload ${BuildConfig.TP_VERSION}\ncounter $counter"
            main.postDelayed(this, 200)
        }
    }

    override fun start(host: HostApi, state: ByteArray?) {
        this.host = host
        if (state != null) counter = JSONObject(String(state)).optLong("counter")
        when (behavior) {
            "throw_start" -> throw IllegalStateException("test payload configured to fail in start()")
            "exit_on_start" -> {
                emit("payload_exiting")
                Process.killProcess(Process.myPid())
            }
        }
        val dcl = dclProbe()
        emit("payload_started", JSONObject()
            .put("counter", counter)
            .put("factory", host.isFactory)
            .put("host_api", host.apiLevel())
            .put("dep_version", Native.depVersion())
            .put("dep_instances", Native.depInstances())
            .put("page_size", Native.pageSize())
            .put("kotlin", KotlinVersion.CURRENT.toString())
            .put("loader", javaClass.classLoader?.javaClass?.name)
            .put("dcl_writable", dcl)
            .put("motd", readMotd(host.slotDir())))
        running = true
        main.post(tick)
        when (behavior) {
            "crash_java" -> main.postDelayed({ throw RuntimeException("test payload crash on the main thread") }, 1000)
            "crash_native" -> thread(name = "payload-native") { Thread.sleep(1000); Native.crash() }
            "own_handler_crash" -> {
                // D30: install our own SIGSEGV handler (which hangs) a moment after start, crash on the main thread later.
                thread(name = "payload-handler") { Thread.sleep(300); Native.installOwnHandler(); emit("own_handler_installed") }
                main.postDelayed({ Native.crash() }, 5000)
            }
            "own_handler_thread" -> {
                // Same, but the crash is on a payload thread (an engine's worker): the main thread stays free for the watchdog.
                thread(name = "payload-handler") { Thread.sleep(300); Native.installOwnHandler(); emit("own_handler_installed") }
                thread(name = "payload-native") { Thread.sleep(5000); Native.crash() }
            }
            "handled_segv" -> {
                // D30 regression: a Mono-like runtime handles a SIGSEGV itself and resumes, then stays healthy.
                thread(name = "payload-handler") { Thread.sleep(300); Native.installHandlingHandler() }
                main.postDelayed({
                    val handled = Native.faultHandled()
                    emit("segv_handled", JSONObject().put("handled", handled))
                    main.postDelayed({ host.markHealthy() }, 500)
                }, 4000)
            }
        }
    }

    /** Try to load a writable copy of our own dex: Android 14+ must refuse. */
    private fun dclProbe(): String = try {
        val src = File(host.slotDir(), "dex/classes.dex")
        val probe = File(host.context().cacheDir, "probe-${System.nanoTime()}.dex")
        src.copyTo(probe, overwrite = true)
        probe.setWritable(true)
        DexClassLoader(probe.path, host.context().codeCacheDir.path, null, javaClass.classLoader).loadClass(javaClass.name)
        probe.delete()
        "loaded"
    } catch (t: Throwable) {
        "refused: ${t.javaClass.simpleName}"
    }

    private fun readMotd(dir: File): String =
        File(dir, "assets/motd.txt").takeIf { it.isFile }?.readText()?.trim() ?: "(none)"

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
        view = TextView(activity).apply { textSize = 20f }
        activity.setContentView(view)
        emit("payload_attached", JSONObject().put("activity", activity.javaClass.name))
        if (behavior == "ok" || behavior == "hot") main.postDelayed({ host.markHealthy() }, 500)
    }

    override fun onContentUpdated(slotDir: File) {
        emit("content_updated", JSONObject().put("motd", readMotd(slotDir)))
        host.markHealthy()
    }

    override fun suspend(): ByteArray {
        running = false
        main.removeCallbacksAndMessages(null)
        emit("payload_suspended", JSONObject().put("counter", counter))
        return JSONObject().put("v", 1).put("counter", counter).toString().toByteArray()
    }
}
