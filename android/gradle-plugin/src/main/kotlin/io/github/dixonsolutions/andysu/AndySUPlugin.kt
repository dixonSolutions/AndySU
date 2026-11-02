package io.github.dixonsolutions.andysu

import com.android.build.api.artifact.SingleArtifact
import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property

/**
 * `andysu { }` in an Android application module.
 *
 * - `role = "host"`: the module builds the frozen host APK. The plugin generates
 *   `assets/andysu/config.json` (from [config]) and `assets/andysu-factory/` (the payload built
 *   into the APK, from [factory], a slot directory produced by `andysu pack --emit-slot`).
 * - `role = "payload"`: the module builds the app itself; `andysuPayload<Variant>` turns its APK
 *   into a payload directory with `andysu split` (dex, native libraries, assets, Java resources
 *   and a descriptor naming [entryClass] or [entryNative]).
 */
abstract class AndySUExtension {
    /** "host" or "payload". */
    abstract val role: Property<String>
    /** Path to the andysu CLI (default: `andysu` on PATH, or the repo's target/ build). */
    abstract val cli: Property<String>
    /** host: config.json for the host (app id, channel, public keys, remote, policy). */
    abstract val config: RegularFileProperty
    /** host: slot directory of the built-in payload. */
    abstract val factory: DirectoryProperty
    /** payload: JVM entry class implementing com.andysu.api.Payload. */
    abstract val entryClass: Property<String>
    /** payload: native entry "libname:symbol". */
    abstract val entryNative: Property<String>
    /** payload: application id of the host (default: this module's namespace). */
    abstract val app: Property<String>
    /** payload: payload version (default: versionName). */
    abstract val version: Property<String>
    /** payload: Host API level the payload needs. */
    abstract val hostApi: Property<Int>
    /** payload: ABIs to keep. */
    abstract val abis: ListProperty<String>
}

class AndySUPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        val ext = project.extensions.create("andysu", AndySUExtension::class.java)
        ext.role.convention("payload")
        ext.hostApi.convention(1)
        ext.cli.convention(project.provider { defaultCli(project) })
        project.plugins.withId("com.android.application") {
            val components = project.extensions.getByType(ApplicationAndroidComponentsExtension::class.java)
            components.onVariants { v ->
                val cap = v.name.replaceFirstChar { it.uppercase() }
                when (ext.role.get()) {
                    "host" -> {
                        val t = project.tasks.register("andysuHostAssets$cap", HostAssetsTask::class.java) {
                            config.set(ext.config)
                            factory.set(ext.factory)
                            outDir.set(project.layout.buildDirectory.dir("generated/andysu/assets/${v.name}"))
                        }
                        v.sources.assets?.addGeneratedSourceDirectory(t, HostAssetsTask::outDir)
                    }
                    "payload" -> project.tasks.register("andysuPayload$cap", PayloadTask::class.java) {
                        group = "andysu"
                        description = "Builds the AndySU payload directory for ${v.name}"
                        apkDir.set(v.artifacts.get(SingleArtifact.APK))
                        loader.set(v.artifacts.getBuiltArtifactsLoader())
                        cli.set(ext.cli)
                        entryClass.set(ext.entryClass)
                        entryNative.set(ext.entryNative)
                        app.set(ext.app.orElse(v.applicationId))
                        version.set(ext.version.orElse(project.provider { v.outputs.firstOrNull()?.versionName?.orNull ?: "0" }))
                        hostApi.set(ext.hostApi)
                        abis.set(ext.abis)
                        outDir.set(project.layout.buildDirectory.dir("andysu/payload/${v.name}"))
                    }
                    else -> error("andysu.role must be \"host\" or \"payload\"")
                }
            }
        }
    }

    private fun defaultCli(project: Project): String {
        val repo = generateSequence(project.rootDir) { it.parentFile }.firstOrNull { java.io.File(it, "core/Cargo.toml").isFile }
        listOf("release", "debug").forEach { p ->
            val f = repo?.resolve("target/$p/andysu")
            if (f != null && f.canExecute()) return f.path
        }
        return "andysu"
    }
}
