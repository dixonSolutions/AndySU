package com.andysu.flutter;

import android.os.Handler;
import android.os.Looper;

import com.andysu.api.Host;
import com.andysu.api.HostApi;

import org.json.JSONObject;

import io.flutter.embedding.engine.plugins.FlutterPlugin;
import io.flutter.plugin.common.EventChannel;
import io.flutter.plugin.common.MethodCall;
import io.flutter.plugin.common.MethodChannel;

/** Flutter bridge to the AndySU host (com.andysu.api.Host). */
public class AndySUPlugin implements FlutterPlugin, MethodChannel.MethodCallHandler, EventChannel.StreamHandler {
    private MethodChannel channel;
    private EventChannel events;

    @Override
    public void onAttachedToEngine(FlutterPluginBinding b) {
        channel = new MethodChannel(b.getBinaryMessenger(), "andysu");
        channel.setMethodCallHandler(this);
        events = new EventChannel(b.getBinaryMessenger(), "andysu/status");
        events.setStreamHandler(this);
    }

    @Override
    public void onDetachedFromEngine(FlutterPluginBinding b) {
        channel.setMethodCallHandler(null);
        events.setStreamHandler(null);
    }

    @Override
    public void onMethodCall(MethodCall call, MethodChannel.Result r) {
        HostApi h = Host.current();
        try {
            switch (call.method) {
                case "info": {
                    JSONObject o = new JSONObject().put("present", h != null);
                    if (h != null) {
                        o.put("apiLevel", h.apiLevel()).put("version", h.version()).put("isFactory", h.isFactory())
                                .put("slotDir", h.slotDir().getPath());
                    }
                    r.success(o.toString());
                    return;
                }
                case "status":
                    r.success(h == null ? "{\"state\":\"idle\"}" : h.updates().status());
                    return;
                case "checkNow":
                    if (h != null) h.updates().checkNow();
                    break;
                case "applyNow":
                    if (h != null) h.updates().applyNow();
                    break;
                case "restartNow":
                    if (h != null) h.updates().restartNow();
                    break;
                case "markHealthy":
                    if (h != null) h.markHealthy();
                    break;
                case "setApplyPolicy":
                    if (h != null) h.updates().setApplyPolicy((String) call.arguments);
                    break;
                default:
                    r.notImplemented();
                    return;
            }
            r.success(null);
        } catch (Exception e) {
            r.error("andysu", e.getMessage(), null);
        }
    }

    @Override
    public void onListen(Object args, EventChannel.EventSink sink) {
        HostApi h = Host.current();
        if (h == null) return;
        Handler main = new Handler(Looper.getMainLooper());
        h.updates().setListener(json -> main.post(() -> sink.success(json)));
    }

    @Override
    public void onCancel(Object args) {
        HostApi h = Host.current();
        if (h != null) h.updates().setListener(null);
    }
}
