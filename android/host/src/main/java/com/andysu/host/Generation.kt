package com.andysu.host

import android.os.Build
import android.util.Log
import com.andysu.api.Payload
import org.json.JSONObject
import java.io.File

/** One loaded payload generation: a JVM payload (class loader + instance) or a native one. */
class Generation private constructor(
    val dir: File,
    val isFactory: Boolean,
    val version: String,
    val payload: Payload?,
    val nativeHandle: Long,
    val loader: ClassLoader?,
) {
    val isNative get() = payload == null
    val api = HostApiImpl(this)
    /** For native generations: state to start with once the GL surface exists. */
    @Volatile var pendingStart: ByteArray? = null
    @Volatile var nativeStarted = false
    @Volatile var pendingNativeStart = false

    override fun toString() = "$version${if (isFactory) " (factory)" else ""}"

    companion object {
        val abi: String get() = Build.SUPPORTED_ABIS.first()

        fun load(dir: File, factory: Boolean): Generation {
            val plan = Core.obj(Core.loadPlan(dir.path, abi))
            val version = plan.optString("version", "?")
            val dexClass = plan.str("dex_class")
            if (dexClass != null) {
                val dex = plan.getJSONArray("dex").let { a -> (0 until a.length()).map { a.getString(it) } }
                require(dex.isNotEmpty()) { "payload names dex_class but ships no dex" }
                val libs = HashMap<String, String>()
                plan.optJSONArray("native")?.let { a ->
                    for (i in 0 until a.length()) {
                        val o: JSONObject = a.getJSONObject(i)
                        libs[o.getString("name").removePrefix("lib")] = o.getString("path")
                    }
                }
                val libDir = File(dir, "lib/$abi").takeIf { it.isDirectory }?.path
                // Before API 26 DexClassLoader needs a writable optimized-dex directory; keep it in the
                // slot so it goes away with the slot.
                val odex = if (Build.VERSION.SDK_INT < 26) File(dir, ".oat").apply { mkdirs() }.path else null
                val dexPath = dex.joinToString(File.pathSeparator)
                var attempt = 1
                while (true) {
                    val loader = PayloadClassLoader(dexPath, odex, libDir, Generation::class.java.classLoader!!, libs)
                    try {
                        val payload = loader.loadClass(dexClass).getDeclaredConstructor().newInstance() as Payload
                        return Generation(dir, factory, version, payload, 0, loader)
                    } catch (e: VerifyError) {
                        // ART verifies a custom loader's classes on its background "Verification thread" too, calling
                        // back into loadClass concurrently with this thread. A class that fails there stays erroneous
                        // in that loader for good, even when the failure was a transient resolution miss (seen once:
                        // java.lang.Object "unresolved"). Class state is per loader, so a fresh loader gets a clean
                        // verdict; a payload that really does not verify fails the same way again and rolls back.
                        if (attempt >= 2) throw e
                        Log.w("AndySU", "payload $version failed verification ($e); retrying with a fresh class loader")
                        AndySU.emit("verify_retry", JSONObject().put("version", version).put("error", e.toString())
                            .put("thread", Thread.currentThread().name))
                        attempt++
                    }
                }
            }
            val entry = plan.str("entry_lib") ?: error("payload has neither dex_class nor native_lib")
            val symbol = plan.str("symbol") ?: error("native payload without symbol")
            val libs = plan.getJSONArray("libs").let { a -> Array(a.length()) { a.getString(it) } }
            val h = NativeBridge.load(libs, entry, symbol, dir.path, version)
            if (h == 0L) throw IllegalStateException("native load failed: ${NativeBridge.lastError()}")
            return Generation(dir, factory, version, null, h, null)
        }
    }
}
