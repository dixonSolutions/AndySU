package com.andysu.godot;

import com.andysu.api.Host;
import com.andysu.api.HostApi;

import org.godotengine.godot.Godot;
import org.godotengine.godot.plugin.GodotPlugin;
import org.godotengine.godot.plugin.SignalInfo;
import org.godotengine.godot.plugin.UsedByGodot;

import java.util.Collections;
import java.util.Set;

/** Engine.get_singleton("AndySU") on Android: a bridge to the AndySU host. */
public class AndySUGodotPlugin extends GodotPlugin {
    public AndySUGodotPlugin(Godot godot) {
        super(godot);
    }

    @Override
    public String getPluginName() {
        return "AndySU";
    }

    @Override
    public Set<SignalInfo> getPluginSignals() {
        return Collections.singleton(new SignalInfo("status_changed", String.class));
    }

    private static HostApi host() {
        return Host.current();
    }

    @UsedByGodot
    public boolean is_present() {
        return host() != null;
    }

    @UsedByGodot
    public String version() {
        return host() == null ? "" : host().version();
    }

    @UsedByGodot
    public String status() {
        return host() == null ? "{\"state\":\"idle\"}" : host().updates().status();
    }

    @UsedByGodot
    public void check_now() {
        if (host() != null) host().updates().checkNow();
    }

    @UsedByGodot
    public void apply_now() {
        if (host() != null) host().updates().applyNow();
    }

    @UsedByGodot
    public void restart_now() {
        if (host() != null) host().updates().restartNow();
    }

    @UsedByGodot
    public void mark_healthy() {
        if (host() != null) host().markHealthy();
    }

    @UsedByGodot
    public void set_apply_policy(String policy) {
        if (host() != null) host().updates().setApplyPolicy(policy == null || policy.isEmpty() ? null : policy);
    }

    // Update mode needs Host API level 3; older hosts lack the methods, so check first.
    private static boolean hasUpdateMode() {
        HostApi h = host();
        return h != null && h.apiLevel() >= 3;
    }

    @UsedByGodot
    public void set_update_mode(String mode) {
        if (hasUpdateMode()) host().updates().setMode(mode == null || mode.isEmpty() ? null : mode);
    }

    @UsedByGodot
    public String update_mode() {
        if (!hasUpdateMode()) return "";
        String m = host().updates().mode();
        return m == null ? "" : m;
    }

    @UsedByGodot
    public void listen() {
        if (host() != null) host().updates().setListener(json -> emitSignal("status_changed", json));
    }
}
