import java.util.zip.ZipFile

plugins {
    id("com.android.application")
    id("io.github.dixonsolutions.andysu")
}

val andysuVersion = providers.gradleProperty("andysu.version").get()

android {
    namespace = "com.example.consumer"
    compileSdk = 36
    defaultConfig {
        applicationId = "com.example.consumer"
        minSdk = 24
        versionName = "1.0"
    }
}

andysu {
    role.set("payload")
    entryClass.set("com.example.consumer.Entry")
}

val hostAar: Configuration by configurations.creating

dependencies {
    compileOnly("io.github.dixonsolutions.andysu:api:$andysuVersion")
    implementation("io.github.dixonsolutions.andysu:payload-kit:$andysuVersion")
    hostAar("io.github.dixonsolutions.andysu:host:$andysuVersion@aar")
}

tasks.register("verifyHostAar") {
    val files = hostAar
    doLast {
        val aar = files.singleFile
        val libs = ZipFile(aar).use { z -> z.entries().toList().map { it.name }.filter { it.endsWith("libandysu_core.so") } }
        check(libs.size == 4) { "expected the core for 4 ABIs in ${aar.name}, found $libs" }
        println("host AAR ${aar.name}: ${libs.joinToString()}")
    }
}
