package com.andysu.host

import android.app.Application

/**
 * Application class for host APKs. Apps with their own Application call `AndySU.init(this)`
 * from onCreate instead (skipping it in the `:andysu_phoenix` process).
 */
open class AndySUApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        if (!Phoenix.isPhoenixProcess()) AndySU.init(this)
    }
}
