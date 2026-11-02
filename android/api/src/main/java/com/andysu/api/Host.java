package com.andysu.api;

/**
 * Process-wide access to the host, for code that is not handed a {@link HostApi} through
 * {@link Payload#start}: framework bindings (Flutter, React Native, Unity, Godot, Capacitor) and
 * apps running in transparent mode. Returns null when the app is not running under an AndySU
 * host (e.g. a plain debug build), so callers can degrade gracefully.
 */
public final class Host {
    private static volatile HostApi current;

    private Host() {}

    public static HostApi current() {
        return current;
    }

    /** Set by the host. Not for payload code. */
    public static void install(HostApi host) {
        current = host;
    }
}
