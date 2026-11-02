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
    }
}

rootProject.name = "andysu-android"

include(":api", ":host", ":payload-kit")
include(":testapp:app", ":testapp:payload")
//include(":samples:compose:app", ":samples:compose:payload")
//include(":samples:native:app", ":samples:native:payload")
//include(":samples:webview:app", ":samples:webview:payload")
