package com.andysu.host

import android.util.AtomicFile
import java.io.File

/** Persistent runtime controls read before class loading in transparent mode. */
internal object UpdateMode {
    private val modes = setOf("auto", "download_only", "check_only", "off")

    private fun atomic(root: File, name: String) = AtomicFile(File(root, name))

    fun effective(root: File, configured: String): String {
        val value = runCatching { atomic(root, "update_mode").openRead().bufferedReader().use { it.readText().trim() } }
            .getOrNull()
        return value?.takeIf { it in modes } ?: configured
    }

    fun set(root: File, mode: String?) {
        val file = atomic(root, "update_mode")
        if (mode == null) {
            file.delete()
            return
        }
        val out = file.startWrite()
        try {
            out.write(mode.toByteArray(Charsets.UTF_8))
            file.finishWrite(out)
        } catch (t: Throwable) {
            file.failWrite(out)
            throw t
        }
    }

    /** Only a matching, one-shot explicit request may promote pending outside auto mode. */
    fun shouldApplyPending(root: File, configured: String): Boolean {
        if (effective(root, configured) == "auto") return true
        val pending = runCatching { Core.obj(Core.state(root.path)).objOrNull("pending")?.optString("id") }
            .getOrNull()?.takeIf { it.isNotBlank() }
        val explicit = runCatching { atomic(root, "apply_pending").openRead().bufferedReader().use { it.readText().trim() } }
            .getOrNull()
        return pending != null && explicit == pending
    }

    /** Clear after the main process has run the launch decision, whether it matched or not. */
    fun consumeApplyPending(root: File) = atomic(root, "apply_pending").delete()

    fun requestApplyPending(root: File) {
        val pending = Core.obj(Core.state(root.path)).objOrNull("pending")?.optString("id")
            ?.takeIf { it.isNotBlank() } ?: return
        val file = atomic(root, "apply_pending")
        val out = file.startWrite()
        try {
            out.write(pending.toByteArray(Charsets.UTF_8))
            file.finishWrite(out)
        } catch (t: Throwable) {
            file.failWrite(out)
            throw t
        }
    }
}
