package io.github.dixonsolutions.andysu

import com.android.build.api.variant.BuiltArtifactsLoader
import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.FileSystemOperations
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.io.File
import javax.inject.Inject

/** Generates the host's assets: andysu/config.json and andysu-factory/. */
abstract class HostAssetsTask @Inject constructor(private val fs: FileSystemOperations) : DefaultTask() {
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val config: RegularFileProperty
    @get:InputDirectory @get:PathSensitive(PathSensitivity.RELATIVE) abstract val factory: DirectoryProperty
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun run() {
        val out = outDir.get().asFile
        fs.delete { delete(out) }
        fs.copy { from(config); into(File(out, "andysu")); rename { "config.json" } }
        fs.copy { from(factory); into(File(out, "andysu-factory")) }
    }
}

/** Runs `andysu split` on the variant's APK to produce the payload directory. */
abstract class PayloadTask @Inject constructor(private val exec: ExecOperations, private val fs: FileSystemOperations) : DefaultTask() {
    @get:InputDirectory @get:PathSensitive(PathSensitivity.RELATIVE) abstract val apkDir: DirectoryProperty
    @get:Internal abstract val loader: Property<BuiltArtifactsLoader>
    @get:Input abstract val cli: Property<String>
    @get:Input @get:Optional abstract val entryClass: Property<String>
    @get:Input @get:Optional abstract val entryNative: Property<String>
    @get:Input abstract val app: Property<String>
    @get:Input abstract val version: Property<String>
    @get:Input abstract val hostApi: Property<Int>
    @get:Input abstract val abis: ListProperty<String>
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun run() {
        val apk = loader.get().load(apkDir.get())?.elements?.singleOrNull()?.outputFile
            ?: error("expected exactly one APK in ${apkDir.get()}")
        val tmp = File(temporaryDir, "split")
        fs.delete { delete(tmp) }
        val args = mutableListOf(cli.get(), "split", apk, "--out", tmp.path, "--app", app.get(), "--version", version.get(), "--host-api", hostApi.get().toString())
        entryClass.orNull?.let { args += listOf("--entry-class", it) }
        entryNative.orNull?.let { args += listOf("--entry-native", it) }
        abis.get().forEach { args += listOf("--abi", it) }
        exec.exec { commandLine(args) }
        val out = outDir.get().asFile
        fs.delete { delete(out) }
        fs.copy { from(File(tmp, "payload")); into(out) }
        logger.lifecycle("AndySU payload: $out")
    }
}
