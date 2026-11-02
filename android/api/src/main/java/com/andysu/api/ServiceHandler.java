package com.andysu.api;

import android.app.Service;
import android.content.Intent;
import android.os.IBinder;

/** Payload code behind one of the host's stub services; register with {@link HostApi#setServiceHandler}. */
public interface ServiceHandler {
    void onCreate(Service service);

    int onStartCommand(Service service, Intent intent, int flags, int startId);

    IBinder onBind(Service service, Intent intent);

    void onDestroy(Service service);
}
