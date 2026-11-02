package com.andysu.host

import android.app.Activity
import android.app.Application
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.util.Log
import com.andysu.api.ActivityEvent
import org.json.JSONObject
import java.io.File
import java.io.FileInputStream
import java.security.SecureRandom

/**
 * The bootloader and generation manager. Frozen in the host APK.
 *
 * Cold start: [ensureBooted] asks the core which slot to run (promoting a staged release into
 * TRIAL, counting trial boots, rolling back crash loops), loads it, and starts it. While on
 * trial a watchdog waits for markHealthy(); an exception from start(), a crash or the watchdog
 * rolls back.
 *
 * Live swap: [applyLive] suspends the running generation (keeping the Activity, window and GL
 * context), loads the staged one in a fresh class loader / with freshly named libraries, and
 * resumes it with the old generation's state. A failure during the live trial restarts the old
 * generation in-process with the same state.
 */
object AndySU {
    const val TAG = "AndySU"
    const val EVENT_TAG = "AndySU-Event"

    lateinit var app: Application
        private set
    lateinit var config: Config
        private set
    lateinit var root: File
        private set
    lateinit var updater: Updater
        private set

    private val main = Handler(Looper.getMainLooper())
    private val rnd = SecureRandom()

    @Volatile var generation: Generation? = null
        private set
    private var previous: Generation? = null
    private var swapState: ByteArray? = null
    @Volatile var inTrial = false
        private set
    private var trialLive = false
    private var booted = false
    /** Transparent mode: the app's own components run from the payload (see Transparent). */
    @Volatile var transparent = false
        private set
    var bootError: String? = null
        private set
    internal val activities = LinkedHashSet<Activity>()
    @Volatile var resumed: Activity? = null
        internal set
    private val watchdog = Runnable { onWatchdog() }

    fun isInitialized() = this::config.isInitialized

    /** Cheap, idempotent; called from Application.onCreate and every host component. */
    @Synchronized
    fun init(context: Context) {
        if (isInitialized()) return
        app = context.applicationContext as Application
        config = Config.load(app)
        root = File(app.noBackupFilesDir, "andysu").apply { mkdirs() }
        updater = Updater(app, config, root)
        installCrashHandlers()
        Jobs.schedule(app, config)
        emit("init", JSONObject().put("core", Core.versionString()).put("sdk", Build.VERSION.SDK_INT).put("abi", Generation.abi))
    }

    fun random16() = ByteArray(16).also { rnd.nextBytes(it) }
    fun nowS() = System.currentTimeMillis() / 1000

    // ---------------------------------------------------------------- events

    fun emit(event: String, data: JSONObject = JSONObject()) {
        data.put("event", event)
        Log.i(EVENT_TAG, data.toString())
    }

    fun emitRaw(json: String) {
        Log.i(EVENT_TAG, json)
    }

    // ---------------------------------------------------------------- boot

    /** Transparent mode start, from InitProvider (before Application.onCreate). */
    internal fun initTransparent(ctx: Context) {
        init(ctx)
        if (booted) return
        booted = true
        transparent = true
        inTrial = Transparent.inTrial
        // The component factory may have been given a copy; point the app's own ApplicationInfo
        // (what getApplicationInfo() returns everywhere) at the slot's libraries too.
        Transparent.slotDir?.let { File(it, "lib/${Generation.abi}") }?.takeIf { it.isDirectory }?.let {
            app.applicationInfo.nativeLibraryDir = it.path
        }
        emit("launch", Transparent.launchEvent ?: JSONObject())
        emit("booted", JSONObject().put("version", Transparent.version).put("factory", Transparent.isFactory)
            .put("trial", inTrial).put("transparent", true))
        com.andysu.api.Host.install(HostApiImpl(null))
        val autoHealthy = config.json.optLong("auto_healthy_ms", 8_000)
        TransparentCallbacks(autoHealthy).installOn(app)
        if (inTrial) {
            // Generous: the app gets until well after its first activity has had time to settle.
            NativeBridge.setRelaunchOnCrash(true)
            main.postDelayed(watchdog, maxOf(config.watchdogMs, autoHealthy + 60_000))
        }
        if (config.checkOnOpen && config.mode != "off") updater.checkAsync("open")
    }

    fun ensureBooted() {
        check(Looper.myLooper() == Looper.getMainLooper())
        if (booted) return
        booted = true
        boot()
        if (config.checkOnOpen && config.mode != "off") updater.checkAsync("open")
    }

    private fun factoryDir(): File = Factory.ensure(app, root)

    private fun boot() {
        var d = Core.obj(Core.launch(root.path, random16(), nowS(), true, config.maxBootAttempts))
        emit("launch", d)
        inTrial = d.optBoolean("in_trial")
        while (true) {
            val dir = d.str("slot_dir")
            val g = try {
                if (dir != null) Generation.load(File(dir), false) else Generation.load(factoryDir(), true)
            } catch (t: Throwable) {
                Log.e(TAG, "load failed", t)
                if (dir != null && inTrial) {
                    d = rollbackInTrial("load failed: $t")
                    continue
                }
                if (dir != null) {
                    bootError = "installed payload failed to load: $t"
                    emit("boot_error", JSONObject().put("error", bootError))
                    return
                }
                bootError = "built-in payload failed to load: $t"
                emit("boot_error", JSONObject().put("error", bootError))
                return
            }
            try {
                startGeneration(g, null)
                generation = g
                break
            } catch (t: Throwable) {
                Log.e(TAG, "start failed", t)
                if (!g.isFactory && inTrial) {
                    d = rollbackInTrial("start() threw: $t")
                    continue
                }
                bootError = "payload ${g.version} failed to start: $t"
                emit("boot_error", JSONObject().put("error", bootError))
                return
            }
        }
        generation?.let { com.andysu.api.Host.install(it.api) }
        emit("booted", JSONObject().put("version", generation?.version).put("factory", generation?.isFactory).put("trial", inTrial))
        if (inTrial) armWatchdog(live = false)
    }

    /** Roll back during a cold-boot trial and return the core's view of what to run instead. */
    private fun rollbackInTrial(reason: String): JSONObject {
        val r = Core.obj(Core.reportFailure(root.path, reason))
        emit("rolled_back", JSONObject().put("reason", reason).put("result", r))
        inTrial = false
        return r
    }

    private fun startGeneration(g: Generation, state: ByteArray?) {
        if (g.isNative) {
            g.pendingStart = state
            g.pendingNativeStart = true
            NativeView.requestStart(g)
        } else {
            g.payload!!.start(g.api, state)
        }
    }

    private fun suspendGeneration(g: Generation): ByteArray? = try {
        if (g.isNative) NativeView.suspendSync(g) else g.payload!!.suspend()
    } catch (t: Throwable) {
        Log.e(TAG, "suspend failed", t)
        null
    }

    // ---------------------------------------------------------------- health

    fun markHealthy() {
        main.post {
            if (!inTrial) return@post
            inTrial = false
            trialLive = false
            main.removeCallbacks(watchdog)
            NativeBridge.setRelaunchOnCrash(false)
            swapState = null
            val r = Core.ok(Core.markHealthy(root.path))
            emit("committed", JSONObject().put("version", generation?.version ?: Transparent.version).put("result", r))
            updater.gcAsync()
        }
    }

    private fun armWatchdog(live: Boolean) {
        trialLive = live
        NativeBridge.setRelaunchOnCrash(true)
        main.removeCallbacks(watchdog)
        main.postDelayed(watchdog, config.watchdogMs)
    }

    private fun onWatchdog() {
        if (!inTrial) return
        emit("watchdog", JSONObject().put("version", generation?.version))
        if (trialLive && previous != null) {
            liveRollback("watchdog: markHealthy not called within ${config.watchdogMs} ms")
        } else {
            Core.ok(Core.reportFailure(root.path, "watchdog: markHealthy not called within ${config.watchdogMs} ms"))
            Phoenix.relaunch(app, "watchdog")
        }
    }

    /** A native generation reported a start failure from the GL thread. */
    internal fun onNativeStartFailed(g: Generation, rc: Int) {
        main.post {
            if (g !== generation) return@post
            val reason = "native start() returned $rc"
            if (trialLive && previous != null) liveRollback(reason)
            else if (inTrial) {
                Core.ok(Core.reportFailure(root.path, reason))
                Phoenix.relaunch(app, reason)
            } else {
                bootError = "payload ${g.version} failed to start: $reason"
                emit("boot_error", JSONObject().put("error", bootError))
                activities.forEach { RecoveryView.show(it) }
            }
        }
    }

    // ---------------------------------------------------------------- live swap

    // ---------------------------------------------------------------- apply policy

    private val policyFile get() = File(root, "apply_policy")
    private var restartWhenBackgrounded = false
    internal var startedActivities = 0

    /** The configured policy, unless the app overrode it at runtime (UpdateClient.setApplyPolicy). */
    fun applyPolicy(): String {
        val p = runCatching { policyFile.readText().trim() }.getOrNull()
        return if (p in setOf("live", "restart", "next_start")) p!! else config.apply
    }

    fun setApplyPolicy(policy: String?) {
        require(policy == null || policy in setOf("live", "restart", "next_start")) { "unknown apply policy $policy" }
        if (policy == null) policyFile.delete() else policyFile.writeText(policy)
        emit("apply_policy", JSONObject().put("policy", applyPolicy()))
    }

    /** Called by the updater after staging, on the main thread. */
    internal fun onStaged(outcome: JSONObject) {
        if (config.mode != "auto") return
        var policy = applyPolicy()
        if (policy == "live" && transparent) policy = config.liveFallback
        when (policy) {
            "live" -> if (resumed != null && !applyLive()) restartToApply()
            "restart" -> restartToApply()
            else -> Unit // next_start: the next cold start promotes it
        }
    }

    /** Apply by restarting the process. With restart_when = background (or when nothing is on
     *  screen) the process just ends once the user has left, and the next start applies it. */
    private fun restartToApply() {
        if (inTrial) return
        if (startedActivities == 0) {
            emit("restart_to_apply", JSONObject().put("when", "background"))
            android.os.Process.killProcess(android.os.Process.myPid())
        } else if (config.restartWhen == "background") {
            restartWhenBackgrounded = true
            emit("restart_to_apply", JSONObject().put("when", "on_background"))
        } else {
            Phoenix.relaunch(app, "applying update")
        }
    }

    /** All activities stopped: finish a deferred restart-to-apply. */
    internal fun onBackgrounded() {
        if (restartWhenBackgrounded && !inTrial) {
            emit("restart_to_apply", JSONObject().put("when", "background"))
            android.os.Process.killProcess(android.os.Process.myPid())
        }
    }

    /** Apply the staged release now, in-process. Returns false if there was nothing to apply. */
    fun applyLive(): Boolean {
        check(Looper.myLooper() == Looper.getMainLooper())
        val old = generation ?: return false
        if (inTrial) return false
        val pending = Core.obj(Core.state(root.path)).objOrNull("pending") ?: return false
        if (pending.optBoolean("hot_only") && !old.isNative) return applyHot(old)
        val state = suspendGeneration(old)
        emit("suspended", JSONObject().put("version", old.version).put("state_bytes", state?.size ?: 0))
        val r = try {
            Core.obj(Core.beginLive(root.path, nowS()))
        } catch (t: Throwable) {
            // Nothing changed on disk: resume the generation we just suspended.
            Log.e(TAG, "beginLive failed", t)
            emit("begin_live_failed", JSONObject().put("error", t.toString()))
            restart(old, state)
            return false
        }
        val dir = r.str("slot_dir")
        previous = old
        swapState = state
        inTrial = true
        val ng = try {
            Generation.load(File(dir!!), false)
        } catch (t: Throwable) {
            Log.e(TAG, "live load failed", t)
            generation = old
            liveRollback("new generation failed to load: $t", suspendCurrent = false)
            return true
        }
        generation = ng
        com.andysu.api.Host.install(ng.api)
        emit("live_swapped", JSONObject().put("from", old.version).put("to", ng.version))
        armWatchdog(live = true)
        try {
            startGeneration(ng, state)
            if (!ng.isNative) activities.toList().forEach { attachPayload(it, null) }
            else activities.toList().forEach { NativeView.attach(it, ng) }
        } catch (t: Throwable) {
            Log.e(TAG, "live start failed", t)
            liveRollback("start() threw: $t")
        }
        return true
    }

    private fun restart(g: Generation, state: ByteArray?) {
        try {
            startGeneration(g, state)
            if (!g.isNative) activities.toList().forEach { attachPayload(it, null) }
            else activities.toList().forEach { NativeView.attach(it, g) }
        } catch (t: Throwable) {
            Log.e(TAG, "restart failed", t)
            Phoenix.relaunch(app, "generation failed to restart")
        }
    }

    private fun applyHot(g: Generation): Boolean {
        val r = Core.obj(Core.beginLive(root.path, nowS()))
        val dir = File(r.str("slot_dir")!!)
        inTrial = true
        previous = g
        emit("hot_applied", JSONObject().put("from", g.version).put("dir", dir.path))
        armWatchdog(live = true)
        try {
            g.payload!!.onContentUpdated(dir)
        } catch (t: Throwable) {
            Log.e(TAG, "content update failed", t)
            liveRollback("onContentUpdated threw: $t", suspendCurrent = false)
            try { g.payload!!.onContentUpdated(g.dir) } catch (_: Throwable) {}
        }
        return true
    }

    private fun liveRollback(reason: String, suspendCurrent: Boolean = true) {
        main.removeCallbacks(watchdog)
        val bad = generation
        val back = previous
        emit("live_rollback", JSONObject().put("reason", reason).put("from", bad?.version).put("to", back?.version))
        if (suspendCurrent && bad != null && bad !== back) suspendGeneration(bad)
        val r = Core.obj(Core.reportFailure(root.path, reason))
        emit("rolled_back", JSONObject().put("reason", reason).put("result", r))
        inTrial = false
        trialLive = false
        NativeBridge.setRelaunchOnCrash(false)
        previous = null
        if (back == null || back === bad) return
        generation = back
        com.andysu.api.Host.install(back.api)
        try {
            startGeneration(back, swapState)
            if (!back.isNative) activities.toList().forEach { attachPayload(it, null) }
            else activities.toList().forEach { NativeView.attach(it, back) }
        } catch (t: Throwable) {
            Log.e(TAG, "restart of previous generation failed", t)
            Phoenix.relaunch(app, "previous generation failed to restart")
        }
        swapState = null
    }

    // ---------------------------------------------------------------- activities

    internal fun attach(activity: Activity, saved: Bundle?) {
        activities += activity
        resumed = activity
        val g = generation
        when {
            g == null -> RecoveryView.show(activity)
            g.isNative -> NativeView.attach(activity, g)
            else -> attachPayload(activity, saved)
        }
    }

    private fun attachPayload(activity: Activity, saved: Bundle?) {
        val g = generation ?: return
        try {
            g.payload!!.onActivityCreated(activity, saved)
        } catch (t: Throwable) {
            Log.e(TAG, "onActivityCreated failed", t)
            if (inTrial && trialLive) liveRollback("onActivityCreated threw: $t") else throw t
        }
    }

    internal fun activityEvent(activity: Activity, event: Int) {
        when (event) {
            ActivityEvent.START -> startedActivities++
            ActivityEvent.STOP -> if (--startedActivities <= 0) {
                startedActivities = 0
                onBackgrounded()
            }
            ActivityEvent.RESUME -> resumed = activity
            ActivityEvent.PAUSE -> if (resumed === activity) resumed = null
            ActivityEvent.DESTROY -> activities -= activity
        }
        val g = generation ?: return
        if (g.isNative) NativeView.onEvent(activity, event) else g.payload!!.onActivityEvent(activity, event)
    }

    // ---------------------------------------------------------------- crashes

    private fun installCrashHandlers() {
        val previousHandler = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { t, e ->
            try {
                emit("uncaught", JSONObject().put("thread", t.name).put("error", e.toString()).put("trial", inTrial)
                    .put("stack", Log.getStackTraceString(e).take(4000)))
                if (inTrial) {
                    Core.ok(Core.reportFailure(root.path, "uncaught exception: $e"))
                    Phoenix.relaunch(app, "uncaught exception during trial")
                    return@setDefaultUncaughtExceptionHandler
                }
            } catch (inner: Throwable) {
                Log.e(TAG, "crash handler failed", inner)
            }
            previousHandler?.uncaughtException(t, e)
        }
        // Native: the signal handler writes the marker and, on a non-main thread during trial,
        // pokes this pipe and parks the thread; we relaunch from here.
        val marker = Core.ok(Core.crashMarkerPath(root.path)) as String
        val pipe = ParcelFileDescriptor.createPipe()
        NativeBridge.installCrashHandler(marker, pipe[1].detachFd())
        Thread({
            val sig = FileInputStream(pipe[0].fileDescriptor).read()
            if (sig > 0) {
                Log.e(TAG, "native crash (signal $sig) during trial, relaunching")
                emit("native_crash", JSONObject().put("signal", sig))
                Phoenix.relaunch(app, "native crash during trial")
            }
        }, "andysu-crash-watch").apply { isDaemon = true }.start()
    }

    /** Recovery: forget installed releases and restart on the built-in payload. */
    fun resetToFactory(reason: String) {
        Core.ok(Core.reset(root.path, reason))
        Phoenix.relaunch(app, "reset to built-in payload")
    }
}
