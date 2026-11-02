package com.andysu.host

import android.content.Context
import android.content.res.AssetManager
import android.os.Build
import java.io.File
import java.io.FileNotFoundException

/**
 * The payload built into the host APK (`assets/andysu-factory/`, a packed slot). Extracted once
 * per host version into `noBackupFilesDir/andysu/factory/<versionCode>/`, code made read-only.
 */
object Factory {
    private const val ASSET_DIR = "andysu-factory"

    @Suppress("DEPRECATION")
    private fun versionCode(ctx: Context): Long {
        val pi = ctx.packageManager.getPackageInfo(ctx.packageName, 0)
        return if (Build.VERSION.SDK_INT >= 28) pi.longVersionCode else pi.versionCode.toLong()
    }

    fun ensure(ctx: Context, root: File): File {
        val base = File(root, "factory")
        val dir = File(base, versionCode(ctx).toString())
        if (File(dir, "andysu-payload.json").isFile) return dir
        val tmp = File(base, "${dir.name}.tmp")
        tmp.deleteRecursively()
        tmp.mkdirs()
        val n = copy(ctx.assets, ASSET_DIR, tmp)
        if (n == 0) throw FileNotFoundException("no assets/$ASSET_DIR in the host APK")
        tmp.walkTopDown().filter { it.isFile && (it.name.endsWith(".dex") || it.name.endsWith(".so")) }.forEach { it.setReadOnly() }
        base.listFiles()?.filter { it.name != tmp.name }?.forEach { old -> old.walkBottomUp().forEach { it.setWritable(true); it.delete() } }
        check(tmp.renameTo(dir)) { "rename factory dir" }
        return dir
    }

    private fun copy(am: AssetManager, path: String, out: File): Int {
        val children = am.list(path) ?: emptyArray()
        if (children.isEmpty()) {
            return try {
                am.open(path).use { i -> out.outputStream().use { i.copyTo(it) } }
                1
            } catch (e: FileNotFoundException) {
                0
            }
        }
        out.mkdirs()
        return children.sumOf { copy(am, "$path/$it", File(out, it)) }
    }
}
