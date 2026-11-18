package com.andysu.host

import org.junit.Assert.assertEquals
import org.junit.Test

/** The phoenix process must not instantiate the app's Application (it lives in the payload). */
class PhoenixApplicationTest {
    private val app = "io.github.dixonsolutions.godnotic.NativeUIApplication"

    @Test fun phoenixProcessGetsThePlatformApplication() {
        assertEquals("android.app.Application", phoenixApplicationClass("io.github.dixonsolutions.godnotic:andysu_phoenix", app))
    }

    @Test fun otherProcessesKeepTheManifestClass() {
        assertEquals(app, phoenixApplicationClass("io.github.dixonsolutions.godnotic", app))
        assertEquals(app, phoenixApplicationClass("io.github.dixonsolutions.godnotic:remote", app))
        assertEquals(app, phoenixApplicationClass("", app))
    }
}
