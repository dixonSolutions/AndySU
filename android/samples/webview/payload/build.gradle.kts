// Sample: a WebView app. The web content (assets/web) is a hot component: updates to it apply
// without a restart (tier A); code changes go through a live swap (tier B).
plugins {
    alias(libs.plugins.android.application)
    id("io.github.dixonsolutions.andysu")
}

android {
    namespace = "com.andysu.sample.webview"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.andysu.sample.webview"
        minSdk = 21
        targetSdk = 36
        versionCode = 1
        versionName = (findProperty("sample.version") as String?) ?: "1.0"
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

andysu {
    role.set("payload")
    entryClass.set("com.andysu.sample.webview.WebPayload")
}

dependencies { compileOnly(project(":api")) }
