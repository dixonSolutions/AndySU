import org.gradle.process.ExecOperations
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.library)
}

val coreAbis = (findProperty("andysu.abis") as String).split(",").map { it.trim() }.filter { it.isNotEmpty() }

/** Builds libandysu_core.so (Rust, JNI glue) for every ABI with cargo-ndk. */
abstract class CargoNdk @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Input abstract val abis: ListProperty<String>
    @get:InputDirectory @get:PathSensitive(PathSensitivity.RELATIVE) abstract val coreSrc: DirectoryProperty
    @get:Internal abstract val repoRoot: DirectoryProperty
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun run() {
        val home = System.getProperty("user.home")
        val cargo = listOf("$home/.cargo/bin/cargo").firstOrNull { File(it).canExecute() } ?: "cargo"
        val args = mutableListOf(cargo, "ndk")
        abis.get().forEach { args += listOf("-t", it) }
        args += listOf("--platform", "21", "-o", outDir.get().asFile.path, "build", "--release", "-p", "andysu-core", "--features", "jni")
        exec.exec {
            workingDir = repoRoot.get().asFile
            commandLine(args)
        }
    }
}

val buildCore = tasks.register<CargoNdk>("buildAndySUCore") {
    abis.set(coreAbis)
    repoRoot.set(rootProject.layout.projectDirectory.dir(".."))
    coreSrc.set(rootProject.layout.projectDirectory.dir("../core/src"))
    outDir.set(layout.buildDirectory.dir("generated/andysu-core/jniLibs"))
}

android {
    namespace = "com.andysu.host"
    compileSdk = 36
    ndkVersion = "27.2.12479018"
    defaultConfig {
        minSdk = 21
        consumerProguardFiles("consumer-rules.pro")
        ndk { abiFilters += coreAbis }
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.22.1"
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

androidComponents {
    onVariants { v -> v.sources.jniLibs?.addGeneratedSourceDirectory(buildCore, CargoNdk::outDir) }
}

dependencies {
    // The one runtime copy of the API lives in the host.
    api(project(":api"))
}
