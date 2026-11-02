import java.util.Random

// The test payload. Behaviour and version are build-time inputs so the device tests can publish
// a sequence of releases (good, throwing, crashing, never healthy, ...).
plugins {
    alias(libs.plugins.android.application)
    id("io.github.dixonsolutions.andysu")
}

val tpVersion = (findProperty("tp.version") as String?) ?: "1.0"
val tpBehavior = (findProperty("tp.behavior") as String?) ?: "ok"
val tpBlobKb = (findProperty("tp.blobKb") as String?) ?: "0"

android {
    namespace = "com.andysu.testpayload"
    compileSdk = 36
    ndkVersion = "27.2.12479018"
    defaultConfig {
        applicationId = "com.andysu.testapp"
        minSdk = 21
        targetSdk = 36
        versionCode = 1
        versionName = tpVersion
        buildConfigField("String", "TP_VERSION", "\"$tpVersion\"")
        buildConfigField("String", "TP_BEHAVIOR", "\"$tpBehavior\"")
        externalNativeBuild { cmake { arguments += listOf("-DTP_VERSION=${tpVersion.substringBefore('.')}") } }
        ndk { abiFilters += (findProperty("andysu.abis") as String).split(",") }
    }
    buildFeatures { buildConfig = true }
    externalNativeBuild { cmake { path = file("src/main/cpp/CMakeLists.txt"); version = "3.22.1" } }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

andysu {
    role.set("payload")
    entryClass.set("com.andysu.testpayload.TestPayload")
    app.set("com.andysu.testapp")
}

// Optional large asset (for interrupted/resumed download tests) plus a per-version motd.txt.
abstract class TestAssets : DefaultTask() {
    @get:Input abstract val kb: Property<Int>
    @get:Input abstract val ver: Property<String>
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun run() {
        val d = outDir.get().asFile
        d.deleteRecursively()
        d.mkdirs()
        if (kb.get() > 0) {
            d.resolve("big").mkdirs()
            d.resolve("big/blob.bin").writeBytes(ByteArray(kb.get() * 1024).also { Random(ver.get().hashCode().toLong()).nextBytes(it) })
        }
        d.resolve("motd.txt").writeText("motd for ${ver.get()}\n")
    }
}
val testAssets = tasks.register<TestAssets>("generateTestAssets") {
    kb.set(tpBlobKb.toInt())
    ver.set(tpVersion)
    outDir.set(layout.buildDirectory.dir("generated/test-assets"))
}
androidComponents {
    onVariants { v -> v.sources.assets?.addGeneratedSourceDirectory(testAssets, TestAssets::outDir) }
}

dependencies {
    compileOnly(project(":api"))
}
