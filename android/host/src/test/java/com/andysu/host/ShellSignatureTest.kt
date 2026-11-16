package com.andysu.host

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ShellSignatureTest {
    @Test fun requiresVerifiedMatchingSigners() {
        val installed = setOf(listOf(1.toByte(), 2.toByte()))
        val other = setOf(listOf(3.toByte()))
        assertTrue(trustedApkSigner(true, installed, installed))
        assertFalse(trustedApkSigner(false, installed, installed))
        assertFalse(trustedApkSigner(true, installed, other))
        assertFalse(trustedApkSigner(true, emptySet(), emptySet()))
    }
}
