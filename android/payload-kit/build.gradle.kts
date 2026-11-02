// Payload-side helpers (compiled into payloads, updated with them): gives a plain host stub
// Activity the AndroidX lifecycle and saved-state owners that Compose and ViewModels expect.
plugins {
    alias(libs.plugins.android.library)
}

android {
    namespace = "com.andysu.kit"
    compileSdk = 36
    defaultConfig { minSdk = 21 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    compileOnly(project(":api"))
    api(libs.lifecycle.runtime)
    api(libs.savedstate)
}
