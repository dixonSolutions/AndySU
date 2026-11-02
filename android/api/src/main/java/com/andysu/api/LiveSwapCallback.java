package com.andysu.api;

/**
 * How an app reports the outcome of {@link LiveSwapHandler#onLiveSwap}. Thread-safe: call it from
 * any thread, once. Host API level 2.
 */
public interface LiveSwapCallback {
    /**
     * The app now runs the new release's native code and content. The host arms its watchdog:
     * call {@link HostApi#markHealthy()} once the new code is known to work, or the release is
     * rolled back (and the app relaunched) when {@code watchdog_ms} pass without it.
     */
    void started();

    /**
     * The swap failed and the app kept (or restored) its previous native code and content and is
     * still running. The host marks the release bad and makes the previous slot current again.
     */
    void failed(String reason);
}
