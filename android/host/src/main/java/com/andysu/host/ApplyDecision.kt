package com.andysu.host

/**
 * What an apply does after its live attempt (pure, unit tested). Restarting is only for a release
 * that cannot be swapped in this process: a swap the app's handler declined, or one blocked by a
 * running trial, stays staged rather than being forced through by relaunching the app under the
 * player.
 */
internal object ApplyDecision {
    const val SWAPPED = "swapped"
    const val DECLINED = "declined"
    const val TRIAL = "trial"
    const val NOT_LIVE = "not_live"
    const val NONE = "none"

    /** After applyNow()'s live attempt: "done", "stay" (keep it staged, say why) or "restart". */
    fun afterLive(result: String): String = when (result) {
        SWAPPED -> "done"
        DECLINED, TRIAL, NONE -> "stay"
        else -> "restart"
    }

    /** onStaged's live_fallback: a restart while an activity is resumed waits for the background. */
    fun fallbackOnBackground(fallback: String, resumed: Boolean): Boolean = fallback == "restart" && resumed
}
