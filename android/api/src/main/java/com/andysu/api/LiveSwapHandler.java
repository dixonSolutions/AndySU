package com.andysu.api;

import java.io.File;

/**
 * Lets an app in transparent mode take a staged release live without restarting the process
 * (Host API level 2; check {@link HostApi#apiLevel()} {@code >= 2} before registering one with
 * {@link HostApi#setLiveSwapHandler}).
 *
 * <p>In transparent mode the app's own Java code runs from the payload under its real names and
 * cannot be replaced in a running process. A release that changes only native libraries and
 * content (no dex, no Java resources; the core reports it as {@code native_live}) can still be
 * applied live if the app can swap those itself, for example an engine behind a resident loader.
 * With {@code apply = "live"} the host calls this handler for such a release; otherwise
 * ({@code live_fallback}) the update waits for the next start as before.
 */
public interface LiveSwapHandler {
    /**
     * Called on the main thread. {@code slotDir} is the new release's slot (read-only code and
     * content), {@code version} its version.
     *
     * <p>Return {@code false} (or throw) to decline: nothing changed, the host applies the
     * fallback policy. Return {@code true} to accept; the swap may then complete asynchronously
     * and the app must report the result through {@code cb}: {@link LiveSwapCallback#started()}
     * or {@link LiveSwapCallback#failed(String)}. The host treats the release as a live trial
     * from the moment it accepts.
     */
    boolean onLiveSwap(File slotDir, String version, LiveSwapCallback cb);
}
