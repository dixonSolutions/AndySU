pluginManagement {
    includeBuild("gradle-plugin")
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // The Flutter embedding, for the Flutter binding's compile check only.
        maven("https://storage.googleapis.com/download.flutter.io") { content { includeGroup("io.flutter") } }
    }
}

rootProject.name = "andysu-android"

include(":api", ":host", ":payload-kit", ":host-template")
include(":testapp:app", ":testapp:payload")
//include(":samples:compose:app", ":samples:compose:payload")
//include(":samples:native:app", ":samples:native:payload")
//include(":samples:webview:app", ":samples:webview:payload")

// The ecosystem bindings' Android libraries, compiled against :api (-Pandysu.bindings=true; CI).
if (providers.gradleProperty("andysu.bindings").orNull == "true") {
    for (b in listOf("react-native", "capacitor", "flutter", "godot")) {
        include(":binding-$b")
        project(":binding-$b").projectDir = file("../bindings/$b/android")
    }
}
