package com.andysu.host

import android.app.Activity
import android.app.Service
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.andysu.api.HostApi
import com.andysu.api.ReceiverHandler
import com.andysu.api.ServiceHandler
import com.andysu.api.UpdateClient
import java.io.File

/** Host API level 3 (levels 1-2 plus persistent update-mode control), one instance per generation (g == null: transparent mode). */
class HostApiImpl internal constructor(private val g: Generation?) : HostApi {
    override fun apiLevel() = LEVEL
    override fun setLiveSwapHandler(handler: com.andysu.api.LiveSwapHandler?) { AndySU.liveSwapHandler = handler }
    override fun context(): Context = AndySU.app
    override fun activity(): Activity? = AndySU.resumed ?: AndySU.activities.lastOrNull()
    override fun slotDir(): File = g?.dir ?: Transparent.slotDir ?: AndySU.root
    override fun version() = g?.version ?: Transparent.version
    override fun isFactory() = g?.isFactory ?: Transparent.isFactory
    override fun markHealthy() {
        if (g == null || AndySU.generation === g) AndySU.markHealthy()
    }
    override fun log(message: String) { Log.i(AndySU.TAG, "payload ${version()}: $message") }
    override fun updates(): UpdateClient = AndySU.updater
    override fun stubActivityCount() = Stubs.activities.size
    override fun stubActivity(index: Int): Class<out Activity> = Stubs.activities[index]
    override fun stubServiceCount() = Stubs.services.size
    override fun stubService(index: Int): Class<out Service> = Stubs.services[index]
    override fun setServiceHandler(index: Int, handler: ServiceHandler?) { Stubs.serviceHandlers[index] = handler }
    override fun stubReceiverCount() = Stubs.receivers.size
    override fun setReceiverHandler(index: Int, handler: ReceiverHandler?) { Stubs.receiverHandlers[index] = handler }
    override fun runOnMain(r: Runnable) {
        if (Looper.myLooper() == Looper.getMainLooper()) r.run() else Handler(Looper.getMainLooper()).post(r)
    }
    override fun core(method: String, argsJson: String?): String {
        val root = AndySU.root.path
        return when (method) {
            "state" -> Core.state(root)
            "version" -> Core.version()
            "load_plan" -> Core.loadPlan(slotDir().path, Generation.abi)
            "gc" -> Core.gc(root, false)
            "etag" -> Core.etag(root)
            else -> """{"error":{"code":"usage","message":"unknown core method $method"}}"""
        }
    }

    companion object {
        /** The Host API level this host AAR implements (levels are additive). */
        const val LEVEL = 3
    }
}
