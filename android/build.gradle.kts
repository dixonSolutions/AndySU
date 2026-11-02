plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

// Release publishing: the libraries go to a static Maven repo (a directory that the release
// workflow merges into GitHub Pages). -Pandysu.mavenRepo=<dir> selects it.
val publishedLibraries = mapOf(
    "api" to "Interfaces shared by an AndySU host and its payloads (compileOnly in payloads)",
    "host" to "AndySU host: bootloader, stubs, updater and the native core for every ABI",
    "payload-kit" to "AndySU payload helpers: AndroidX lifecycle and saved-state owners for stub activities",
)
subprojects {
    val desc = publishedLibraries[name] ?: return@subprojects
    plugins.withId("com.android.library") {
        apply(plugin = "maven-publish")
        extensions.configure<com.android.build.api.dsl.LibraryExtension> {
            publishing { singleVariant("release") { withSourcesJar() } }
        }
        afterEvaluate {
            extensions.configure<PublishingExtension> {
                publications.create<MavenPublication>("release") {
                    from(components["release"])
                    groupId = property("GROUP") as String
                    artifactId = project.name
                    version = property("VERSION_NAME") as String
                    pom {
                        name.set("AndySU ${project.name}")
                        description.set(desc)
                        url.set("https://github.com/dixonSolutions/AndySU")
                        licenses { license { name.set("Apache-2.0"); url.set("https://www.apache.org/licenses/LICENSE-2.0") } }
                        scm { url.set("https://github.com/dixonSolutions/AndySU") }
                    }
                }
                (findProperty("andysu.mavenRepo") as String?)?.let { dir ->
                    repositories { maven { name = "site"; url = uri(dir) } }
                }
            }
        }
    }
}
