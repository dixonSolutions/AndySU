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

    /**
     * How staged updates are applied, overriding the host's configured policy for this install
     * (persisted): {@code "live"} (swap in-process; the default), {@code "restart"} (restart the
     * app to apply), {@code "next_start"} (wait for the next start), or null to go back to the
     * configured policy.
     */
    void setApplyPolicy(String policy);

    /** The policy in effect: "live", "restart" or "next_start". */
    String applyPolicy();

    /**
     * Override how much automatic updating this install performs (persisted): {@code "auto"},
     * {@code "download_only"}, {@code "check_only"}, or {@code "off"}. A null value clears the
     * override and returns to the mode configured in the host. {@code off} also cancels the
     * periodic job and prevents an in-flight check from staging or automatically applying a
     * release. Explicit {@link #applyNow()} remains available for an update already staged.
     *
     * <p>This method was added in Host API level 3. Check {@link HostApi#apiLevel()} before using
     * it when supporting hosts built with older API levels.</p>
     */
    void setMode(String mode);

    /** The effective mode: "auto", "download_only", "check_only", or "off". Host API level 3. */
    String mode();
}
