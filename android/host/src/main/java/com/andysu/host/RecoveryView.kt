package com.andysu.host

import android.app.Activity
import android.view.Gravity
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView

/** Shown when no payload can run. Part of the frozen host: plain platform views, no resources. */
object RecoveryView {
    fun show(a: Activity) {
        val pad = (16 * a.resources.displayMetrics.density).toInt()
        val status = TextView(a).apply { text = "" }
        val layout = LinearLayout(a).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(pad, pad, pad, pad)
            addView(TextView(a).apply {
                text = "This app could not start.\n\n${AndySU.bootError ?: ""}"
                textSize = 16f
            })
            addView(Button(a).apply {
                text = "Check for a fix"
                setOnClickListener {
                    status.text = "Checking…"
                    AndySU.updater.checkAsync("recovery")
                }
            })
            addView(Button(a).apply {
                text = "Use the built-in version"
                setOnClickListener { AndySU.resetToFactory("user chose the built-in version") }
            })
            addView(status)
        }
        AndySU.updater.recoveryStatus = { s -> a.runOnUiThread { status.text = s } }
        a.setContentView(layout)
    }
}
