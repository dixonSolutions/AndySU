package com.andysu.host

import android.annotation.SuppressLint
import android.app.Activity
import android.opengl.GLSurfaceView
import com.andysu.api.ActivityEvent
import java.util.WeakHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/**
 * Host-owned GL surface for native payloads. The EGL context and the surface belong to the host
 * and survive live swaps; payload start/frame/suspend run on this GL thread with the context
 * current. While a swap is in progress nothing is rendered, so the last frame stays on screen.
 */
@SuppressLint("ViewConstructor")
class NativeView(private val activity: Activity) : GLSurfaceView(activity), GLSurfaceView.Renderer {
    @Volatile var gen: Generation? = null
    private var w = 0
    private var h = 0
    private var surfaceSeen = false

    init {
        setEGLContextClientVersion(2)
        preserveEGLContextOnPause = true
        setRenderer(this)
        renderMode = RENDERMODE_CONTINUOUSLY
    }

    override fun onSurfaceCreated(gl: GL10?, config: EGLConfig?) {
        val g = gen
        if (surfaceSeen && g != null && g.nativeStarted) {
            // The EGL context was lost: restart the payload on the new context with its own state.
            g.pendingStart = NativeBridge.suspend(g.nativeHandle)
            g.nativeStarted = false
            g.pendingNativeStart = true
        }
        surfaceSeen = true
    }

    override fun onSurfaceChanged(gl: GL10?, width: Int, height: Int) {
        w = width
        h = height
        startPending()
    }

    override fun onDrawFrame(gl: GL10?) {
        val g = gen ?: return
        if (g.pendingNativeStart) startPending()
        if (g.nativeStarted) NativeBridge.frame(g.nativeHandle, w, h)
    }

    private fun startPending() {
        val g = gen ?: return
        if (!g.pendingNativeStart || w == 0) return
        g.pendingNativeStart = false
        val rc = NativeBridge.start(g.nativeHandle, g.pendingStart, activity, w, h)
        g.pendingStart = null
        if (rc == 0) g.nativeStarted = true else AndySU.onNativeStartFailed(g, rc)
    }

    companion object {
        private val views = WeakHashMap<Activity, NativeView>()

        fun attach(a: Activity, g: Generation) {
            val v = views.getOrPut(a) { NativeView(a).also { a.setContentView(it) } }
            v.gen = g
            v.renderMode = RENDERMODE_CONTINUOUSLY
        }

        /** Nothing to do until a view shows the generation; kept for symmetry with JVM payloads. */
        fun requestStart(@Suppress("UNUSED_PARAMETER") g: Generation) {}

        fun suspendSync(g: Generation): ByteArray? {
            if (!g.nativeStarted) return g.pendingStart
            val v = views.values.firstOrNull { it.gen === g }
            if (v == null) {
                g.nativeStarted = false
                return NativeBridge.suspend(g.nativeHandle)
            }
            v.renderMode = RENDERMODE_WHEN_DIRTY // freeze on the last frame during the swap
            var out: ByteArray? = null
            val done = CountDownLatch(1)
            v.queueEvent {
                g.nativeStarted = false
                out = NativeBridge.suspend(g.nativeHandle)
                done.countDown()
            }
            if (!done.await(3, TimeUnit.SECONDS)) AndySU.emit("suspend_timeout")
            return out
        }

        fun onEvent(a: Activity, event: Int) {
            val v = views[a] ?: return
            when (event) {
                ActivityEvent.PAUSE -> v.onPause()
                ActivityEvent.RESUME -> v.onResume()
                ActivityEvent.DESTROY -> views.remove(a)
            }
        }
    }
}
