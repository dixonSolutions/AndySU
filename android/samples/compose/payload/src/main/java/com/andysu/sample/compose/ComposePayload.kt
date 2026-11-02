package com.andysu.sample.compose

import android.app.Activity
import android.os.Bundle
import android.util.Log
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.unit.dp
import com.andysu.api.HostApi
import com.andysu.api.PayloadAdapter
import com.andysu.api.UpdateClient
import com.andysu.kit.ActivityOwners
import org.json.JSONObject

class ComposePayload : PayloadAdapter() {
    private lateinit var host: HostApi
    private var owners: ActivityOwners? = null
    private var taps by mutableIntStateOf(0)
    private var status by mutableStateOf("idle")

    override fun start(host: HostApi, state: ByteArray?) {
        this.host = host
        if (state != null) taps = JSONObject(String(state)).optInt("taps")
        host.updates().setListener(UpdateClient.Listener { s -> status = JSONObject(s).optString("state") })
        Log.i("AndySU-Event", JSONObject().put("event", "sample_started").put("sample", "compose").put("version", host.version()).toString())
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
        owners?.destroy()
        val o = ActivityOwners(activity, savedInstanceState).also { owners = it }
        val view = ComposeView(activity)
        o.install(view)
        view.setContent { Screen() }
        activity.setContentView(view)
    }

    override fun onActivityEvent(activity: Activity, event: Int) { owners?.onEvent(event) }

    override fun suspend(): ByteArray {
        owners?.destroy()
        owners = null
        host.updates().setListener(null)
        return JSONObject().put("v", 1).put("taps", taps).toString().toByteArray()
    }

    @androidx.compose.runtime.Composable
    private fun Screen() {
        LaunchedEffect(Unit) { host.markHealthy() } // first composition worked
        MaterialTheme {
            Surface(Modifier.fillMaxSize()) {
                Column(Modifier.safeDrawingPadding().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("AndySU Compose sample", style = MaterialTheme.typography.headlineSmall)
                    Text("Payload version ${host.version()}${if (host.isFactory()) " (built in)" else ""}")
                    Text("Taps survive live updates: $taps")
                    Button(onClick = { taps++ }) { Text("Tap") }
                    Button(onClick = { host.updates().checkNow() }) { Text("Check for updates") }
                    Text("Updater: $status")
                }
            }
        }
    }
}
