package com.andysu.reactnative;

import com.andysu.api.Host;
import com.andysu.api.HostApi;
import com.facebook.react.bridge.Promise;
import com.facebook.react.bridge.ReactApplicationContext;
import com.facebook.react.bridge.ReactContextBaseJavaModule;
import com.facebook.react.bridge.ReactMethod;
import com.facebook.react.modules.core.DeviceEventManagerModule;

import org.json.JSONObject;

/** NativeModules.AndySU: a thin bridge to the AndySU host (com.andysu.api.Host). */
public class AndySUModule extends ReactContextBaseJavaModule {
    public AndySUModule(ReactApplicationContext ctx) {
        super(ctx);
    }

    @Override
    public String getName() {
        return "AndySU";
    }

    @ReactMethod
    public void info(Promise p) {
        HostApi h = Host.current();
        try {
            JSONObject o = new JSONObject().put("present", h != null);
            if (h != null) {
                o.put("apiLevel", h.apiLevel()).put("version", h.version()).put("isFactory", h.isFactory())
                        .put("slotDir", h.slotDir().getPath());
            }
            p.resolve(o.toString());
        } catch (Exception e) {
            p.reject("andysu", e);
        }
    }

    @ReactMethod
    public void status(Promise p) {
        HostApi h = Host.current();
        p.resolve(h == null ? "{\"state\":\"idle\"}" : h.updates().status());
    }

    @ReactMethod
    public void checkNow() {
        HostApi h = Host.current();
        if (h != null) h.updates().checkNow();
    }

    @ReactMethod
    public void applyNow() {
        HostApi h = Host.current();
        if (h != null) h.updates().applyNow();
    }

    @ReactMethod
    public void restartNow() {
        HostApi h = Host.current();
        if (h != null) h.updates().restartNow();
    }

    @ReactMethod
    public void setApplyPolicy(String policy) {
        HostApi h = Host.current();
        if (h != null) h.updates().setApplyPolicy(policy);
    }

    @ReactMethod
    public void applyPolicy(Promise p) {
        HostApi h = Host.current();
        p.resolve(h == null ? "live" : h.updates().applyPolicy());
    }

    // Update mode needs Host API level 3; older hosts lack the methods, so check first.
    @ReactMethod
    public void setUpdateMode(String mode) {
        HostApi h = Host.current();
        if (h != null && h.apiLevel() >= 3) h.updates().setMode(mode);
    }

    @ReactMethod
    public void updateMode(Promise p) {
        HostApi h = Host.current();
        String m = (h != null && h.apiLevel() >= 3) ? h.updates().mode() : null;
        p.resolve(m == null ? "" : m);
    }

    @ReactMethod
    public void markHealthy() {
        HostApi h = Host.current();
        if (h != null) h.markHealthy();
    }

    @ReactMethod
    public void listen() {
        HostApi h = Host.current();
        if (h == null) return;
        h.updates().setListener(json -> getReactApplicationContext()
                .getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter.class)
                .emit("AndySUStatus", json));
    }

    // Required by NativeEventEmitter on the JS side.
    @ReactMethod
    public void addListener(String eventName) {}

    @ReactMethod
    public void removeListeners(double count) {}
}
