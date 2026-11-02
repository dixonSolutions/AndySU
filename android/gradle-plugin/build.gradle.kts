import java.util.Properties

plugins {
    `kotlin-dsl`
    `java-gradle-plugin`
    `maven-publish`
}

group = "io.github.dixonsolutions.andysu"
// The version lives in ../gradle.properties (stamped by scripts/set-version.sh); this is an included build.
version = (findProperty("VERSION_NAME") as String?)
    ?: Properties().apply { file("../gradle.properties").reader().use { load(it) } }.getProperty("VERSION_NAME")

dependencies {
    compileOnly("com.android.tools.build:gradle-api:${libs.versions.agp.get()}")
}

gradlePlugin {
    plugins {
        create("andysu") {
            id = "io.github.dixonsolutions.andysu"
            implementationClass = "io.github.dixonsolutions.andysu.AndySUPlugin"
            displayName = "AndySU"
            description = "Splits an Android app into a frozen host APK and a self-updating payload"
        }
    }
}

publishing {
    (findProperty("andysu.mavenRepo") as String?)?.let { dir ->
        repositories { maven { name = "site"; url = uri(dir) } }
    }
}
