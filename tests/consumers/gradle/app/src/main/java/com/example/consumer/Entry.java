package com.example.consumer;

import com.andysu.api.HostApi;
import com.andysu.api.PayloadAdapter;

/** Compiles against the published API: the payload entry of a consumer app. */
public class Entry extends PayloadAdapter {
    @Override
    public void start(HostApi host, byte[] state) {
        host.markHealthy();
    }
}
