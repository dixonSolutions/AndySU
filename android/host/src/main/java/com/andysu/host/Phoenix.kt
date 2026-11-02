package com.andysu.host

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Process
import org.json.JSONObject
import java.io.File
import kotlin.system.exitProcess

/**
 * Clean process restart: start [PhoenixActivity] in its own process (allowed, we are in the
 * foreground), die, and let it relaunch the app. The user sees one relaunch. At most 3
 * relaunches per minute, so a payload that keeps crashing cannot loop forever.
 */
object Phoenix {
    const val EXTRA_PID = "andysu.pid"
    const val EXTRA_RESTART = "andysu.restart"

    private fun allowed(ctx: Context): Boolean {
        val f = File(ctx.noBackupFilesDir, "andysu/relaunches")
        val now = System.currentTimeMillis()
        val recent = (if (f.exists()) f.readLines().mapNotNull { it.toLongOrNull() } else emptyList()).filter { now - it < 60_000 }
        f.parentFile?.mkdirs()
        f.writeText((recent + now).joinToString("\n"))
        return recent.size < 3
    }

    fun relaunch(ctx: Context, reason: String) {
        AndySU.emit("relaunch", JSONObject().put("reason", reason))
        if (!allowed(ctx)) {
            AndySU.emit("relaunch_suppressed", JSONObject().put("reason", "too many relaunches"))
            Process.killProcess(Process.myPid())
            exitProcess(10)
        }
        val restart = ctx.packageManager.getLaunchIntentForPackage(ctx.packageName) ?: Intent(ctx, HostActivity::class.java)
        restart.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
        ctx.startActivity(
            Intent(ctx, PhoenixActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(EXTRA_PID, Process.myPid())
                .putExtra(EXTRA_RESTART, restart),
        )
        Process.killProcess(Process.myPid())
        exitProcess(0)
    }

    fun isPhoenixProcess(): Boolean {
        val name = if (Build.VERSION.SDK_INT >= 28) android.app.Application.getProcessName()
        else runCatching { File("/proc/self/cmdline").readText().trimEnd('\u0000') }.getOrDefault("")
        return name.endsWith(":andysu_phoenix")
    }
}

/** Runs in the `:andysu_phoenix` process; finishes the old process and starts the app again. */
class PhoenixActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pid = intent.getIntExtra(Phoenix.EXTRA_PID, -1)
        if (pid > 0 && pid != Process.myPid()) Process.killProcess(pid)
        @Suppress("DEPRECATION")
        val restart = intent.getParcelableExtra<Intent>(Phoenix.EXTRA_RESTART)
        if (restart != null) startActivity(restart)
        finish()
        Runtime.getRuntime().exit(0)
    }
}
