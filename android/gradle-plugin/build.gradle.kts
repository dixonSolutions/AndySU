plugins {
    `kotlin-dsl`
    `java-gradle-plugin`
    `maven-publish`
}

group = "io.github.dixonsolutions.andysu"
version = (findProperty("VERSION_NAME") as String?) ?: "0.1.0"

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
