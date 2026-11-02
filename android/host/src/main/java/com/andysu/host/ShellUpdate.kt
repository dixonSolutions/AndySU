package com.andysu.host

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import org.json.JSONObject
import java.io.File
import java.security.MessageDigest

/**
 * Tier C: replace the host APK itself. Optional and never silent unless the platform allows it:
 * the app must declare REQUEST_INSTALL_PACKAGES itself (the AAR does not), the user must allow
 * "install unknown apps" for it (API 26+), and Android shows a confirmation unless the app
 * installed itself and API >= 31 (setRequireUserAction). The APK is verified against the signed
 * manifest's sha256 before it is handed to PackageInstaller.
 */
object ShellUpdate {
    const val ACTION = "com.andysu.host.SHELL_INSTALL_STATUS"

    fun available(ctx: Context): Boolean {
        val req = ctx.packageManager.getPackageInfo(ctx.packageName, PackageManager.GET_PERMISSIONS).requestedPermissions ?: emptyArray()
        return "android.permission.REQUEST_INSTALL_PACKAGES" in req
    }

    fun install(ctx: Context, status: JSONObject) {
        if (!available(ctx)) {
            AndySU.emit("shell_update_unavailable", JSONObject().put("reason", "REQUEST_INSTALL_PACKAGES not declared"))
            return
        }
        val sha = status.getString("sha256")
        val size = status.getLong("size")
        val urls = status.getJSONArray("urls")
        val apk = File(AndySU.root, "share/shell-${sha.take(12)}.apk").apply { parentFile?.mkdirs() }
        var ok = false
        for (i in 0 until urls.length()) {
            try {
                Http.download(urls.getString(i), apk, 0, size)
                ok = Core.ok(Core.verifyFile(apk.path, sha, size)) == null
                if (ok) break
            } catch (e: Exception) {
                AndySU.emit("shell_download_failed", JSONObject().put("error", e.toString()))
            }
        }
        if (!ok) return
        val pi = ctx.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        if (Build.VERSION.SDK_INT >= 31) params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        val id = pi.createSession(params)
        pi.openSession(id).use { s ->
            s.openWrite("base.apk", 0, apk.length()).use { out -> apk.inputStream().use { it.copyTo(out) }; s.fsync(out) }
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            val intent = Intent(ACTION).setPackage(ctx.packageName)
            s.commit(PendingIntent.getBroadcast(ctx, id, intent, flags).intentSender)
        }
        AndySU.emit("shell_install_committed", JSONObject().put("session", id))
    }
}

/** Receives PackageInstaller status; launches the confirmation UI when the platform requires it. */
class ShellInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val st = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        if (st == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            @Suppress("DEPRECATION")
            (intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT))?.let {
                context.startActivity(it.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
        }
        if (AndySU.isInitialized()) AndySU.emit("shell_install_status", JSONObject().put("status", st).put("message", intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)))
    }
}
