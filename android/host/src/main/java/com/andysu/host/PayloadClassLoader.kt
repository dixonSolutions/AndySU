package com.andysu.host

import android.util.Log
import dalvik.system.DexClassLoader

/**
 * Child-first loader for one payload generation. The payload's own copies of libraries (Kotlin,
 * AndroidX, Compose...) win over anything in the host, so a host frozen years ago never shadows
 * them. Only `com.andysu.api` (the boundary) and the platform come from the parent.
 *
 * `System.loadLibrary("game")` in payload code resolves to the generation's uniquely renamed
 * `libgame.<hash>.so` through [findLibrary]; the library search path is the slot's lib/<abi>
 * directory, so renamed DT_NEEDED entries resolve inside the same generation.
 */
class PayloadClassLoader(
    dexPath: String,
    /** Optimized-dex directory; required before API 26 (ignored from then on), null otherwise. */
    optimizedDir: String?,
    librarySearchPath: String?,
    parent: ClassLoader,
    private val libraries: Map<String, String>,
) : DexClassLoader(dexPath, optimizedDir, librarySearchPath, parent) {

    override fun loadClass(name: String, resolve: Boolean): Class<*> {
        try {
            findLoadedClass(name)?.let { return it }
            if (name.startsWith("com.andysu.api.") || name.startsWith("java.") || name.startsWith("android.") ||
                name.startsWith("dalvik.") || name.startsWith("javax.")
            ) {
                return super.loadClass(name, resolve)
            }
            return try {
                findClass(name)
            } catch (e: ClassNotFoundException) {
                super.loadClass(name, resolve)
            }
        } catch (e: ClassNotFoundException) {
            throw e
        } catch (t: Throwable) {
            // ART calls loadClass from its verifier (also on a background thread) and silently treats any throw
            // as "class not found"; leave a trace so a verification failure like that can be explained.
            Log.w("AndySU", "PayloadClassLoader.loadClass($name) threw on ${Thread.currentThread()?.name}: $t")
            throw t
        }
    }

    override fun findLibrary(name: String): String? = libraries[name] ?: super.findLibrary(name)
}
