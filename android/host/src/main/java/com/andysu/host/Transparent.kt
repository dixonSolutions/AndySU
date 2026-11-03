package com.andysu.host

import android.annotation.SuppressLint
import android.app.Activity
import android.app.AppComponentFactory
import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.res.AssetFileDescriptor
import android.content.res.loader.AssetsProvider
import android.content.res.loader.ResourcesLoader
import android.content.res.loader.ResourcesProvider
import android.database.Cursor
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.util.Log
import android.annotation.TargetApi
import org.json.JSONObject
import java.io.File
import java.security.SecureRandom
import java.util.zip.ZipFile

/**
 * Transparent mode (Android 9+): the app's own components, under their real names, run from the
 * payload. The platform asks [AndySUComponentFactory] for the app's class loader before any app
 * class is loaded; we answer with the class loader of the slot the core chose. Native code finds
 * its libraries through `ApplicationInfo.nativeLibraryDir`, which points into the slot, and the
 * payload's assets reach `getAssets()` through a ResourcesLoader (Android 11+).
 *
 * Updates apply on the next start. A generation is committed once an activity has stayed resumed
 * for `auto_healthy_ms` without a crash.
 */
object Transparent {
    @Volatile var active = false
        private set
    @Volatile var slotDir: File? = null
        private set
    @Volatile var inTrial = false
        private set
    @Volatile var isFactory = false
        private set
    @Volatile var version: String = "?"
        private set
    @Volatile var launchEvent: JSONObject? = null
        private set
    /** The slot this process booted from (null for the built-in payload). A live swap leaves it on disk. */
    @Volatile var bootSlotId: String? = null
        private set

    /** A live native swap (D29) moved the app to another slot; the process keeps its class loader. */
    internal fun setSlot(dir: File, newVersion: String, factory: Boolean) {
        slotDir = dir
        version = newVersion
        isFactory = factory
    }

    private fun processName(): String = if (Build.VERSION.SDK_INT >= 28) Application.getProcessName() else ""

    /** Factory payload straight from the APK (no Context exists yet). */
    private fun factoryFromApk(apk: String, root: File): File {
        ZipFile(apk).use { z ->
            val desc = z.getEntry("assets/andysu-factory/andysu-payload.json") ?: error("no built-in payload in the APK")
            val dir = File(root, "factory/t-${java.lang.Long.toHexString(desc.crc)}-${desc.size}")
            if (File(dir, "andysu-payload.json").isFile) return dir
            val tmp = File(root, "factory/${dir.name}.tmp")
            tmp.deleteRecursively()
            val prefix = "assets/andysu-factory/"
            for (e in z.entries()) {
                if (e.isDirectory || !e.name.startsWith(prefix)) continue
                val rel = e.name.removePrefix(prefix)
                if (rel.split('/').any { it == ".." || it.isEmpty() }) continue
                val out = File(tmp, rel)
                out.parentFile?.mkdirs()
                z.getInputStream(e).use { i -> out.outputStream().use { i.copyTo(it) } }
                out.setReadOnly()
            }
            File(root, "factory").listFiles()?.filter { it.name != tmp.name }?.forEach { old ->
                old.walkBottomUp().forEach { it.setWritable(true); it.delete() }
            }
            check(tmp.renameTo(dir)) { "rename factory dir" }
            return dir
        }
    }

    private fun config(apk: String): JSONObject = ZipFile(apk).use { z ->
        JSONObject(z.getInputStream(z.getEntry("assets/andysu/config.json")).use { it.readBytes().decodeToString() })
    }

    /** Called once per process by the component factory. Returns null to keep the APK's loader. */
    fun boot(parent: ClassLoader, ai: ApplicationInfo): ClassLoader? {
        val proc = processName()
        if (proc.endsWith(":andysu_phoenix")) return null
        val root = File(ai.dataDir, "no_backup/andysu").apply { mkdirs() }
        val cfg = config(ai.sourceDir)
        val maxAttempts = cfg.optInt("max_boot_attempts", 2)
        val r16 = ByteArray(16).also { SecureRandom().nextBytes(it) }
        val now = System.currentTimeMillis() / 1000
        // Only the main process moves the state machine; other processes follow its choice.
        var d = if (proc.isEmpty() || proc == ai.packageName) Core.obj(Core.launch(root.path, r16, now, true, maxAttempts))
        else JSONObject().put("slot_dir", Core.obj(Core.state(root.path)).objOrNull("current")?.let { File(root, "slots/" + it.getString("id")).path })
        launchEvent = d
        while (true) {
            val trial = d.optBoolean("in_trial")
            val dirPath = d.str("slot_dir")
            val dir = dirPath?.let { File(it) } ?: factoryFromApk(ai.sourceDir, root)
            try {
                val plan = Core.obj(Core.loadPlan(dir.path, Generation.abi))
                val dex = plan.getJSONArray("dex").let { a -> (0 until a.length()).map { a.getString(it) } }
                require(dex.isNotEmpty()) { "payload has no dex" }
                val libDir = File(dir, "lib/${Generation.abi}")
                val libs = HashMap<String, String>()
                plan.optJSONArray("native")?.let { a ->
                    for (i in 0 until a.length()) a.getJSONObject(i).let { libs[it.getString("name").removePrefix("lib")] = it.getString("path") }
                }
                val loader = PayloadClassLoader(dex.joinToString(File.pathSeparator), null, libDir.takeIf { it.isDirectory }?.path, parent, libs)
                if (libDir.isDirectory) ai.nativeLibraryDir = libDir.path
                slotDir = dir
                bootSlotId = if (dirPath != null && dir.parentFile?.name == "slots") dir.name else null
                inTrial = trial
                isFactory = dirPath == null
                version = plan.optString("version", "?")
                active = true
                return loader
            } catch (t: Throwable) {
                Log.e(AndySU.TAG, "transparent load of $dir failed", t)
                if (dirPath != null && trial) {
                    d = Core.obj(Core.reportFailure(root.path, "transparent load failed: $t"))
                    continue
                }
                if (dirPath != null) {
                    d = JSONObject() // fall back to the built-in payload
                    continue
                }
                throw t
            }
        }
    }

    /** Payload assets for getAssets(), Android 11+. Asset listing is not supported. */
    @TargetApi(30)
    fun assetsLoader(dir: File): ResourcesLoader {
        val base = File(dir, "assets").canonicalFile
        val provider = object : AssetsProvider {
            override fun loadAssetFd(path: String, accessMode: Int): AssetFileDescriptor? {
                val rel = path.removePrefix("assets/")
                val f = File(base, rel).canonicalFile
                if (!f.path.startsWith(base.path + File.separator) || !f.isFile) return null
                val pfd = ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY)
                return AssetFileDescriptor(pfd, 0, AssetFileDescriptor.UNKNOWN_LENGTH)
            }
        }
        return ResourcesLoader().apply { addProvider(ResourcesProvider.empty(provider)) }
    }
}

@TargetApi(28)
class AndySUComponentFactory : AppComponentFactory() {
    override fun instantiateClassLoader(cl: ClassLoader, aInfo: ApplicationInfo): ClassLoader = try {
        Transparent.boot(cl, aInfo) ?: cl
    } catch (t: Throwable) {
        Log.e(AndySU.TAG, "AndySU transparent boot failed; running the APK's own code", t)
        cl
    }
}

/**
 * Starts AndySU in transparent mode: providers are created after the Application is attached and
 * before Application.onCreate.
 */
class InitProvider : ContentProvider() {
    @SuppressLint("NewApi")
    override fun onCreate(): Boolean {
        val ctx = context ?: return true
        if (!Transparent.active) return true
        AndySU.initTransparent(ctx)
        return true
    }
    override fun query(u: Uri, p: Array<out String>?, s: String?, a: Array<out String>?, o: String?): Cursor? = null
    override fun getType(uri: Uri): String? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, s: String?, a: Array<out String>?) = 0
    override fun update(uri: Uri, v: ContentValues?, s: String?, a: Array<out String>?) = 0
}

/** Activity tracking for transparent mode: assets loader, auto-healthy, resumed activity. */
internal class TransparentCallbacks(private val autoHealthyMs: Long) : Application.ActivityLifecycleCallbacks {
    private val main = Handler(Looper.getMainLooper())
    private var loader: Any? = if (Build.VERSION.SDK_INT >= 30) Transparent.slotDir?.let { Transparent.assetsLoader(it) } else null
    private val healthy = Runnable { if (AndySU.resumed != null) AndySU.markHealthy() }

    fun installOn(app: Application) {
        if (Build.VERSION.SDK_INT >= 30 && loader != null) app.resources.addLoaders(loader as ResourcesLoader)
        app.registerActivityLifecycleCallbacks(this)
    }

    /** A live native swap (D29): `getAssets()` must now see the new slot's assets (Android 11+). */
    @SuppressLint("NewApi")
    fun switchAssets(app: Application, dir: File) {
        if (Build.VERSION.SDK_INT < 30) return
        val old = loader as? ResourcesLoader
        val new = Transparent.assetsLoader(dir)
        val all = (listOf(app.resources) + AndySU.activities.map { it.resources }).distinct()
        for (r in all) {
            if (old != null) r.removeLoaders(old)
            r.addLoaders(new)
        }
        loader = new
    }

    override fun onActivityPreCreated(activity: Activity, savedInstanceState: Bundle?) {
        if (Build.VERSION.SDK_INT >= 30 && loader != null) activity.resources.addLoaders(loader as ResourcesLoader)
    }
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) { AndySU.activities += activity }
    override fun onActivityStarted(activity: Activity) { AndySU.onStarted() }
    override fun onActivityResumed(activity: Activity) {
        AndySU.resumed = activity
        if (AndySU.inTrial) {
            main.removeCallbacks(healthy)
            main.postDelayed(healthy, autoHealthyMs)
        }
    }
    override fun onActivityPaused(activity: Activity) {
        if (AndySU.resumed === activity) AndySU.resumed = null
        main.removeCallbacks(healthy)
    }
    override fun onActivityStopped(activity: Activity) {
        if (--AndySU.startedActivities <= 0) {
            AndySU.startedActivities = 0
            AndySU.onBackgrounded()
        }
    }
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
    override fun onActivityDestroyed(activity: Activity) { AndySU.activities -= activity }
}
