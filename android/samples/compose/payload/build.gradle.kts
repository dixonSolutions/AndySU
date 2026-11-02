// Sample: a single-Activity Jetpack Compose app shipped as an AndySU payload. Compose, AndroidX
// and Kotlin all live in the payload, so they update with it; the host APK never changes.
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    id("io.github.dixonsolutions.andysu")
}

android {
    namespace = "com.andysu.sample.compose"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.andysu.sample.compose"
        minSdk = 23
        targetSdk = 36
        versionCode = 1
        versionName = (findProperty("sample.version") as String?) ?: "1.0"
    }
    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

andysu {
    role.set("payload")
    entryClass.set("com.andysu.sample.compose.ComposePayload")
}

dependencies {
    compileOnly(project(":api"))
    implementation(project(":payload-kit"))
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
}
