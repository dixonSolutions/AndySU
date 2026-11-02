package com.andysu.api;

import android.app.Activity;
import android.content.res.Configuration;
import android.os.Bundle;

import java.io.File;

/** No-op {@link Payload}; override what you need. */
public abstract class PayloadAdapter implements Payload {
    @Override
    public byte[] suspend() {
        return null;
    }

    @Override
    public void onActivityCreated(Activity activity, Bundle savedInstanceState) {}

    @Override
    public void onActivityEvent(Activity activity, int event) {}

    @Override
    public boolean onBackPressed(Activity activity) {
        return false;
    }

    @Override
    public void onConfigurationChanged(Activity activity, Configuration newConfig) {}

    @Override
    public void onContentUpdated(File slotDir) {}
}
