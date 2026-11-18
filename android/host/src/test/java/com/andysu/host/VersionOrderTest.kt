package com.andysu.host

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VersionOrderTest {
    @Test fun numericParts() {
        assertTrue(VersionOrder.newer("1.2.286", "1.2.285"))
        assertTrue(VersionOrder.newer("1.10.0", "1.9.9"))
        assertTrue(VersionOrder.newer("1.2.1", "1.2"))
        assertFalse(VersionOrder.newer("1.2.285", "1.2.285"))
        assertFalse(VersionOrder.newer("1.2.276", "1.2.285"))
    }
}
