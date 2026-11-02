package com.andysu.api;

import android.app.Activity;
import android.content.res.Configuration;
import android.os.Bundle;

import java.io.File;

/**
 * Entry point of a JVM payload (Host API level 1). The class named in {@code andysu-payload.json}
 * ({@code entry.dex_class}) must implement this interface and have a public no-argument
 * constructor. Extend {@link PayloadAdapter} to implement only what you need.
 *
 * <p>One instance exists per payload generation. The host calls {@link #start} once, then
 * delivers Activity callbacks for its stub activities. Before a live swap it calls
 * {@link #suspend}; the returned bytes are given to the next generation's {@link #start}.
 *
 * <p>This interface never changes. New capabilities arrive as new interfaces in later Host API
 * levels; check {@link HostApi#apiLevel()} before using them.
 */
public interface Payload {
    /**
     * Called on the main thread. Throwing marks this generation as failed and rolls back.
     *
     * @param state bytes returned by the previous generation's {@link #suspend()}, or null on a
     *     cold start. Use a versioned, language-neutral encoding (JSON, protobuf), never Java
     *     serialization: the previous generation's classes are gone.
     */
    void start(HostApi host, byte[] state) throws Exception;

    /**
     * Release threads, GL objects, audio and file handles and return state for the next
     * generation (may be null). Called on the main thread.
     */
    byte[] suspend();

    /** A host stub Activity was created (or re-attached after a live swap). Set its content here. */
    void onActivityCreated(Activity activity, Bundle savedInstanceState);

    /** Lifecycle of a stub Activity; {@code event} is one of the {@link ActivityEvent} constants. */
    void onActivityEvent(Activity activity, int event);

    /** Return true if the payload handled the back press. */
    boolean onBackPressed(Activity activity);

    void onConfigurationChanged(Activity activity, Configuration newConfig);

    /** Content-only ("hot") update applied: re-read files under {@code slotDir}. */
    void onContentUpdated(File slotDir);
}
