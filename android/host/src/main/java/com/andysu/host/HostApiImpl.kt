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

/** Host API level 1, one instance per generation. */
class HostApiImpl internal constructor(private val g: Generation) : HostApi {
    override fun apiLevel() = 1
    override fun context(): Context = AndySU.app
    override fun activity(): Activity? = AndySU.resumed ?: AndySU.activities.lastOrNull()
    override fun slotDir(): File = g.dir
    override fun version() = g.version
    override fun isFactory() = g.isFactory
    override fun markHealthy() {
        if (AndySU.generation === g) AndySU.markHealthy()
    }
    override fun log(message: String) { Log.i(AndySU.TAG, "payload ${g.version}: $message") }
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
            "load_plan" -> Core.loadPlan(g.dir.path, Generation.abi)
            "gc" -> Core.gc(root, false)
            "etag" -> Core.etag(root)
            else -> """{"error":{"code":"usage","message":"unknown core method $method"}}"""
        }
    }
}
