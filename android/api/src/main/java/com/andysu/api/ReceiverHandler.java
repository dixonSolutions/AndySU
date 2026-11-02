package com.andysu.api;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

/** Payload code behind one of the host's stub receivers; register with {@link HostApi#setReceiverHandler}. */
public interface ReceiverHandler {
    void onReceive(BroadcastReceiver receiver, Context context, Intent intent);
}
