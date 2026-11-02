// Consumer verification: a fresh project that takes AndySU only from our static Maven repo.
// -Pandysu.repo=<base>/maven -Pandysu.version=<version>
val repo = providers.gradleProperty("andysu.repo").get()
pluginManagement {
    val repo = providers.gradleProperty("andysu.repo").get()
    repositories {
        maven { url = uri(repo); isAllowInsecureProtocol = repo.startsWith("http:") }
        google()
        mavenCentral()
        gradlePluginPortal()
    }
    plugins {
        id("io.github.dixonsolutions.andysu") version providers.gradleProperty("andysu.version").get()
    }
}
dependencyResolutionManagement {
    repositories {
        maven { url = uri(repo); isAllowInsecureProtocol = repo.startsWith("http:") }
        google()
        mavenCentral()
    }
}
rootProject.name = "andysu-consumer"
include(":app")
