package com.andysu.api;

/** Lifecycle events delivered through {@link Payload#onActivityEvent}. */
public final class ActivityEvent {
    public static final int START = 1;
    public static final int RESUME = 2;
    public static final int PAUSE = 3;
    public static final int STOP = 4;
    public static final int DESTROY = 5;
    public static final int WINDOW_FOCUS_GAINED = 6;
    public static final int WINDOW_FOCUS_LOST = 7;

    private ActivityEvent() {}
}
