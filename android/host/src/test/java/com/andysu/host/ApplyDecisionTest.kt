package com.andysu.host

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ApplyDecisionTest {
    @Test fun declinedOrBlockedLiveSwapsStayStaged() {
        assertEquals("done", ApplyDecision.afterLive(ApplyDecision.SWAPPED))
        // Host API 3 relaunched the app here: an app that said "not now" was closed anyway.
        assertEquals("stay", ApplyDecision.afterLive(ApplyDecision.DECLINED))
        assertEquals("stay", ApplyDecision.afterLive(ApplyDecision.TRIAL))
        assertEquals("stay", ApplyDecision.afterLive(ApplyDecision.NONE))
        // A release that cannot be swapped in-process still applies by restarting, as before.
        assertEquals("restart", ApplyDecision.afterLive(ApplyDecision.NOT_LIVE))
    }

    @Test fun liveFallbackRestartWaitsForTheBackground() {
        assertTrue(ApplyDecision.fallbackOnBackground("restart", resumed = true))
        assertFalse(ApplyDecision.fallbackOnBackground("restart", resumed = false))
        assertFalse(ApplyDecision.fallbackOnBackground("next_start", resumed = true))
    }
}
