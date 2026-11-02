package com.andysu.testapp

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.andysu.host.AndySU
import com.andysu.host.Core
import org.json.JSONObject

/** Test-only hooks driven over adb broadcasts. Not part of the AAR. */
class TestCommands : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        AndySU.init(context)
        when (intent.action) {
            "check" -> AndySU.updater.checkAsync("test")
            "apply" -> AndySU.applyLive()
            "state" -> AndySU.emit("state", JSONObject().put("state", JSONObject(Core.state(AndySU.root.path))))
            "gc" -> AndySU.updater.gcAsync()
        }
    }
}
