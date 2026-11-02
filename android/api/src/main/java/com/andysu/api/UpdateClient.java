package com.andysu.api;

/**
 * Update control for payload UI. Status values are JSON objects, for example
 * {@code {"state":"staged","release":"1.4.0","hot_only":false,"mandatory":false}}. States:
 * idle, checking, up_to_date, downloading, staged, applying, shell_update, error.
 */
public interface UpdateClient {
    interface Listener {
        /** Called on the main thread. */
        void onStatus(String statusJson);
    }

    /** Check now (and download/apply according to the configured mode). */
    void checkNow();

    /** Apply a staged update now: live if the payload supports it, otherwise by restarting. */
    void applyNow();

    /** Restart into the newest staged or committed payload. */
    void restartNow();

    /** Current status as JSON. */
    String status();

    /** Replace the listener (null to remove). */
    void setListener(Listener listener);
}
