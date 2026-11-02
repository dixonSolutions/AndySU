// Sample: a pure native (C, OpenGL ES 2) payload. The host owns the surface and EGL context and
// calls frame() on its GL thread; a live update swaps libgame.<hash>.so under the same context.
plugins {
    alias(libs.plugins.android.application)
    id("io.github.dixonsolutions.andysu")
}

val sampleVersion = (findProperty("sample.version") as String?) ?: "1.0"

android {
    namespace = "com.andysu.sample.nativegl"
    compileSdk = 36
    ndkVersion = "27.2.12479018"
    defaultConfig {
        applicationId = "com.andysu.sample.nativegl"
        minSdk = 21
        targetSdk = 36
        versionCode = 1
        versionName = sampleVersion
        externalNativeBuild { cmake { arguments += listOf("-DSAMPLE_VERSION=$sampleVersion") } }
        ndk { abiFilters += (findProperty("andysu.abis") as String).split(",") }
    }
    externalNativeBuild { cmake { path = file("src/main/cpp/CMakeLists.txt"); version = "3.22.1" } }
}

andysu {
    role.set("payload")
    entryNative.set("libgame:andysu_payload_v1")
}
