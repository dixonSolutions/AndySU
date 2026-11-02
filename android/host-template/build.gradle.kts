// The prebuilt host used by `andysu split --host-template` for apps not built with Gradle
// (Flutter, Unity, Godot, React Native, Cordova, NDK...). It is a minimal app whose only code is
// the AndySU host; `andysuHostTemplate` zips its dex and native libraries.
plugins {
    alias(libs.plugins.android.application)
}

android {
    namespace = "com.andysu.hosttemplate"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.andysu.hosttemplate"
        minSdk = 21
        targetSdk = 36
        versionCode = 1
        versionName = (findProperty("VERSION_NAME") as String?) ?: "0.1.0"
        ndk { abiFilters += (findProperty("andysu.abis") as String).split(",") }
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            signingConfig = signingConfigs.getByName("debug") // the template's signature is discarded
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies { implementation(project(":host")) }

val templateZip = tasks.register<Zip>("andysuHostTemplate") {
    group = "andysu"
    description = "Packages the host dex and native libraries for andysu split"
    dependsOn("assembleRelease")
    val apk = layout.buildDirectory.file("outputs/apk/release/host-template-release.apk")
    from(apk.map { zipTree(it) }) {
        include("classes*.dex", "lib/**/libandysu_*.so")
    }
    from(layout.buildDirectory.file("template.json"))
    archiveFileName.set("andysu-host-template.zip")
    destinationDirectory.set(layout.buildDirectory.dir("andysu"))
    doFirst {
        layout.buildDirectory.file("template.json").get().asFile.writeText(
            """{"version":"${project.findProperty("VERSION_NAME") ?: "0.1.0"}","host_api":1}""" + "\n",
        )
    }
}
