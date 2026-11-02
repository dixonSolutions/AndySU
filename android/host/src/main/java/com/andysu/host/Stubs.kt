package com.andysu.host

import android.app.Activity
import android.app.Service
import android.content.BroadcastReceiver
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import android.os.IBinder
import android.os.ParcelFileDescriptor
import com.andysu.api.ActivityEvent
import com.andysu.api.ReceiverHandler
import com.andysu.api.ServiceHandler
import java.io.File
import java.io.FileNotFoundException

/**
 * Stub Activity 0 (the launcher; apps may subclass it). Every callback goes to the running
 * payload generation; the Activity, its window and any GL context survive live swaps.
 */
open class HostActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        AndySU.init(this)
        AndySU.ensureBooted()
        AndySU.attach(this, savedInstanceState)
    }
    override fun onStart() { super.onStart(); AndySU.activityEvent(this, ActivityEvent.START) }
    override fun onResume() { super.onResume(); AndySU.activityEvent(this, ActivityEvent.RESUME) }
    override fun onPause() { AndySU.activityEvent(this, ActivityEvent.PAUSE); super.onPause() }
    override fun onStop() { AndySU.activityEvent(this, ActivityEvent.STOP); super.onStop() }
    override fun onDestroy() { AndySU.activityEvent(this, ActivityEvent.DESTROY); super.onDestroy() }
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        AndySU.activityEvent(this, if (hasFocus) ActivityEvent.WINDOW_FOCUS_GAINED else ActivityEvent.WINDOW_FOCUS_LOST)
    }
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        AndySU.generation?.payload?.onConfigurationChanged(this, newConfig)
    }
    @Deprecated("Deprecated in Java")
    override fun onBackPressed() {
        if (AndySU.generation?.payload?.onBackPressed(this) != true) {
            @Suppress("DEPRECATION")
            super.onBackPressed()
        }
    }
}

/** Landscape stub. */
class StubActivity1 : HostActivity()
/** singleTask stub. */
class StubActivity2 : HostActivity()
/** Translucent stub. */
class StubActivity3 : HostActivity()

open class StubService(private val index: Int) : Service() {
    private val h: ServiceHandler? get() = Stubs.serviceHandlers[index]
    override fun onCreate() {
        super.onCreate()
        AndySU.init(this)
        h?.onCreate(this)
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int =
        h?.onStartCommand(this, intent, flags, startId) ?: START_NOT_STICKY.also { stopSelf(startId) }
    override fun onBind(intent: Intent): IBinder? = h?.onBind(this, intent)
    override fun onDestroy() { h?.onDestroy(this); super.onDestroy() }
}
class StubService0 : StubService(0)
class StubService1 : StubService(1)

open class StubReceiver(private val index: Int) : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        AndySU.init(context)
        Stubs.receiverHandlers[index]?.onReceive(this, context, intent)
    }
}
class StubReceiver0 : StubReceiver(0)
class StubReceiver1 : StubReceiver(1)

/**
 * Read-only file sharing for payloads (`content://<package>.andysu.files/<name>` serves
 * `noBackupFilesDir/andysu/share/<name>`), a dependency-free stand-in for androidx FileProvider.
 */
class StubFileProvider : ContentProvider() {
    override fun onCreate() = true
    private fun file(uri: Uri): File {
        val base = File(context!!.noBackupFilesDir, "andysu/share").canonicalFile
        val f = File(base, uri.lastPathSegment ?: throw FileNotFoundException()).canonicalFile
        if (f.parentFile != base || !f.isFile) throw FileNotFoundException(uri.toString())
        return f
    }
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor =
        ParcelFileDescriptor.open(file(uri), ParcelFileDescriptor.MODE_READ_ONLY)
    override fun getType(uri: Uri): String = if (uri.toString().endsWith(".apk")) "application/vnd.android.package-archive" else "application/octet-stream"
    override fun query(u: Uri, p: Array<out String>?, s: String?, a: Array<out String>?, o: String?): Cursor? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, s: String?, a: Array<out String>?) = 0
    override fun update(uri: Uri, v: ContentValues?, s: String?, a: Array<out String>?) = 0
}

object Stubs {
    val activities: List<Class<out Activity>> = listOf(HostActivity::class.java, StubActivity1::class.java, StubActivity2::class.java, StubActivity3::class.java)
    val services: List<Class<out Service>> = listOf(StubService0::class.java, StubService1::class.java)
    val receivers: List<Class<out BroadcastReceiver>> = listOf(StubReceiver0::class.java, StubReceiver1::class.java)
    val serviceHandlers = arrayOfNulls<ServiceHandler>(2)
    val receiverHandlers = arrayOfNulls<ReceiverHandler>(2)
}
