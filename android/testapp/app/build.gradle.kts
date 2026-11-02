// The test host APK: the AndySU host plus test hooks. Config and factory payload are inputs
// (-Pandysu.config=..., -Pandysu.factory=...) produced by android/tests/device_tests.py.
plugins {
    alias(libs.plugins.android.application)
    id("io.github.dixonsolutions.andysu")
}

android {
    namespace = "com.andysu.testapp"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.andysu.testapp"
        minSdk = 21
        targetSdk = 36
        versionCode = 1
        versionName = "host-1"
        ndk { abiFilters += (findProperty("andysu.abis") as String).split(",") }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

andysu {
    role.set("host")
    config.set(file((findProperty("andysu.config") as String?) ?: "missing-config.json"))
    factory.set(file((findProperty("andysu.factory") as String?) ?: "missing-factory"))
}

dependencies {
    implementation(project(":host"))
}
