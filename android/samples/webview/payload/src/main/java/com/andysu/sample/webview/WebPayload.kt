package com.andysu.sample.webview

import android.annotation.SuppressLint
import android.app.Activity
import android.os.Bundle
import android.util.Log
import android.webkit.WebView
import android.webkit.WebViewClient
import com.andysu.api.HostApi
import com.andysu.api.PayloadAdapter
import org.json.JSONObject
import java.io.File

class WebPayload : PayloadAdapter() {
    private lateinit var host: HostApi
    private var web: WebView? = null
    private var dir: File? = null

    private fun emit(event: String, o: JSONObject = JSONObject()) =
        Log.i("AndySU-Event", o.put("event", event).put("sample", "webview").put("version", host.version()).toString())

    override fun start(host: HostApi, state: ByteArray?) {
        this.host = host
        dir = host.slotDir()
        emit("sample_started")
    }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
        val w = WebView(activity)
        w.settings.javaScriptEnabled = true
        @Suppress("DEPRECATION")
        w.settings.allowFileAccess = true
        w.webViewClient = object : WebViewClient() {
            override fun onPageFinished(view: WebView, url: String) {
                emit("page_loaded", JSONObject().put("title", view.title))
                host.markHealthy()
            }
        }
        web = w
        activity.setContentView(w)
        load()
    }

    private fun load() {
        web?.loadUrl("file://" + File(dir, "assets/web/index.html").path)
    }

    override fun onContentUpdated(slotDir: File) {
        dir = slotDir
        emit("content_updated")
        load()
    }

    override fun suspend(): ByteArray? {
        web?.destroy()
        web = null
        return null
    }
}
