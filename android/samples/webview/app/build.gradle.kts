// The frozen host APK of the WebView sample. Build the payload first, then:
//   ./gradlew :samples:webview:app:assembleDebug -Pandysu.config=<config.json> -Pandysu.factory=<slot dir>
// (samples/build.sh does all of it with a throwaway key).
plugins {
    alias(libs.plugins.android.application)
    id("io.github.dixonsolutions.andysu")
}

android {
    namespace = "com.andysu.sample.webview.host"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.andysu.sample.webview"
        minSdk = 21
        targetSdk = 36
        versionCode = 1
        versionName = "host-1"
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

dependencies { implementation(project(":host")) }
