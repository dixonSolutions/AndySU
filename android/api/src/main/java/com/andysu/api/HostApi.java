package com.andysu.api;

import android.app.Activity;
import android.app.Service;
import android.content.Context;

import java.io.File;

/**
 * What the frozen host offers a payload. Host API levels are additive: level N+1 adds methods
 * through a new interface and never removes any. Exactly one copy of this class exists at
 * runtime, the host's; payloads compile against it with {@code compileOnly}.
 */
public interface HostApi {
    /** Host API level implemented by this host (1 for this interface). */
    int apiLevel();

    /** The application context. */
    Context context();

    /** The resumed (or most recently created) stub Activity, or null. */
    Activity activity();

    /** Absolute path of this generation's slot. Code files in it are read-only. */
    File slotDir();

    /** Payload version from andysu-payload.json. */
    String version();

    /** True when running the payload built into the host APK (no update installed yet). */
    boolean isFactory();

    /**
     * Commit this generation. Call once the payload is known to work (e.g. first frame drawn,
     * first screen shown). Until then it is on trial and a crash or a timeout rolls it back.
     */
    void markHealthy();

    void log(String message);

    /** The update client (check, download, apply). */
    UpdateClient updates();

    /** Number of stub activities the host declares; see {@link #stubActivity}. */
    int stubActivityCount();

    /** Stub Activity class {@code index} (0 is the launcher), to start with an Intent. */
    Class<? extends Activity> stubActivity(int index);

    int stubServiceCount();

    Class<? extends Service> stubService(int index);

    /** Route stub service {@code index} to payload code (null to clear). */
    void setServiceHandler(int index, ServiceHandler handler);

    int stubReceiverCount();

    /** Route stub receiver {@code index} (declared without intent filters) to payload code. */
    void setReceiverHandler(int index, ReceiverHandler handler);

    /** Run on the main thread (immediately if already on it). */
    void runOnMain(Runnable r);

    /**
     * Raw access to the AndySU core (JSON in, JSON out), for payloads that ship their own updater
     * logic. {@code method} is a core API name such as "state" or "load_plan".
     */
    String core(String method, String argsJson);
}
