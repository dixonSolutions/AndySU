package com.andysu.capacitor;

import com.andysu.api.Host;
import com.andysu.api.HostApi;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;

import org.json.JSONException;

/** Capacitor bridge to the AndySU host (com.andysu.api.Host). */
@CapacitorPlugin(name = "AndySU")
public class AndySUPlugin extends Plugin {
    private static HostApi host() {
        return Host.current();
    }

    @PluginMethod
    public void info(PluginCall call) {
        HostApi h = host();
        JSObject o = new JSObject();
        o.put("present", h != null);
        if (h != null) {
            o.put("apiLevel", h.apiLevel());
            o.put("version", h.version());
            o.put("isFactory", h.isFactory());
            o.put("slotDir", h.slotDir().getPath());
        }
        call.resolve(o);
    }

    @PluginMethod
    public void status(PluginCall call) {
        HostApi h = host();
        try {
            call.resolve(new JSObject(h == null ? "{\"state\":\"idle\"}" : h.updates().status()));
        } catch (JSONException e) {
            call.reject(e.getMessage());
        }
    }

    @PluginMethod
    public void checkNow(PluginCall call) {
        if (host() != null) host().updates().checkNow();
        call.resolve();
    }

    @PluginMethod
    public void applyNow(PluginCall call) {
        if (host() != null) host().updates().applyNow();
        call.resolve();
    }

    @PluginMethod
    public void restartNow(PluginCall call) {
        if (host() != null) host().updates().restartNow();
        call.resolve();
    }

    @PluginMethod
    public void markHealthy(PluginCall call) {
        if (host() != null) host().markHealthy();
        call.resolve();
    }

    @PluginMethod
    public void setApplyPolicy(PluginCall call) {
        if (host() != null) host().updates().setApplyPolicy(call.getString("policy"));
        call.resolve();
    }

    @PluginMethod
    public void applyPolicy(PluginCall call) {
        JSObject o = new JSObject();
        o.put("policy", host() == null ? "live" : host().updates().applyPolicy());
        call.resolve(o);
    }

    @Override
    public void load() {
        HostApi h = host();
        if (h != null) {
            h.updates().setListener(json -> {
                try {
                    notifyListeners("status", new JSObject(json));
                } catch (JSONException ignored) {
                }
            });
        }
    }
}
