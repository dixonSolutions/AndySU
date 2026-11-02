# The host is entered from native code (JNI), from the manifest, and reflectively by payloads.
-keep class com.andysu.host.** { *; }
-keep class com.andysu.api.** { *; }
