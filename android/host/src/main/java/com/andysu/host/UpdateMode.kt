package com.andysu.host

import java.io.File
import java.io.FileOutputStream

/** Persistent runtime controls read before class loading in transparent mode. */
internal object UpdateMode {
    private val modes = setOf("auto", "download_only", "check_only", "off")

    private fun file(root: File, name: String) = File(root, name)

    /**
     * Write a small preference atomically without `AtomicFile`: some Android images refuse
     * `renameTo` when the destination already exists, which silently lost the override. Write a
     * synced temporary file, remove the old file, then rename. A crash in the short delete window
     * leaves no file, so [effective] falls back to the configured mode instead of reading a torn
     * value.
     */
    private fun write(root: File, name: String, value: String) {
        val dest = file(root, name)
        val tmp = file(root, "$name.tmp")
        FileOutputStream(tmp).use { out ->
            out.write(value.toByteArray(Charsets.UTF_8))
            out.fd.sync()
        }
        if (dest.exists() && !dest.delete()) {
            tmp.delete()
            throw IllegalStateException("could not replace $dest")
        }
        if (!tmp.renameTo(dest)) {
            tmp.delete()
            throw IllegalStateException("could not rename $tmp to $dest")
        }
    }

    fun effective(root: File, configured: String): String {
        val value = runCatching { file(root, "update_mode").readText().trim() }.getOrNull()
        return value?.takeIf { it in modes } ?: configured
    }

    fun set(root: File, mode: String?) {
        if (mode == null) {
            file(root, "update_mode").delete()
            return
        }
        write(root, "update_mode", mode)
    }

    /** Only a matching, one-shot explicit request may promote pending outside auto mode. */
    fun shouldApplyPending(root: File, configured: String): Boolean {
        if (effective(root, configured) == "auto") return true
        val pending = runCatching { Core.obj(Core.state(root.path)).objOrNull("pending")?.optString("id") }
            .getOrNull()?.takeIf { it.isNotBlank() }
        val explicit = runCatching { file(root, "apply_pending").readText().trim() }.getOrNull()
        return pending != null && explicit == pending
    }

    /** Clear after the main process has run the launch decision, whether it matched or not. */
    fun consumeApplyPending(root: File) {
        file(root, "apply_pending").delete()
    }

    fun requestApplyPending(root: File) {
        val pending = Core.obj(Core.state(root.path)).objOrNull("pending")?.optString("id")
            ?.takeIf { it.isNotBlank() } ?: return
        write(root, "apply_pending", pending)
    }
}
