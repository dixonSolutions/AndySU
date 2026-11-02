#!/usr/bin/env python3
"""The samples (Compose, native GLES, WebView) on an emulator, each built by samples/build.sh:

  1. the host APK starts its built-in payload 1.0 (with nothing but normal permissions)
  2. it finds 2.0 on the remote, applies it live (apply = "live") and commits it
  3. a screenshot of each version goes to docs/evidence/

Usage: python3 android/tests/sample_tests.py [--only compose|native|webview] [--api 34] [--device SERIAL]
"""
import argparse
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import device_tests as dt  # noqa: E402

SAMPLES = {
    "compose": "com.andysu.sample.compose",
    "native": "com.andysu.sample.nativegl",
    "webview": "com.andysu.sample.webview",
}
EVIDENCE = os.path.join(dt.REPO, "docs", "evidence")


def screenshot(d, name):
    os.makedirs(EVIDENCE, exist_ok=True)
    p = os.path.join(EVIDENCE, name + ".png")
    with open(p, "wb") as f:
        subprocess.run([dt.ADB, "-s", d.serial, "exec-out", "screencap", "-p"], stdout=f, timeout=60)
    dt.log(f"screenshot {p}")


def sample(d, name, pkg, results):
    port = dt.free_port()
    dt.log(f"[{name}] building (remote 127.0.0.1:{port})")
    dt.run(["sh", os.path.join(dt.ANDROID, "samples", "build.sh"), name, f"http://127.0.0.1:{port}"], timeout=3600)
    out = os.path.join(dt.ANDROID, "samples", "build", name)
    srv = subprocess.Popen([dt.ANDYSU, "serve", os.path.join(out, "site"), "--addr", f"127.0.0.1:{port}"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        d.adb("uninstall", pkg, check=False)
        d.install(os.path.join(out, "host.apk"))
        perms = d.sh(f"dumpsys package {pkg} | grep -E 'android.permission' | sort -u", check=False)
        dangerous = [p for p in ("CAMERA", "LOCATION", "STORAGE", "INSTALL_PACKAGES", "CONTACTS") if p in perms]
        if dangerous:
            raise dt.Fail(f"{name}: unexpected permissions {dangerous}")
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        d.watch()
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
        ev = d.wait(lambda e: e.get("event") == "booted" and e.get("version") == "1.0", f"{name} 1.0 booted", 120)
        if not ev.get("factory"):
            raise dt.Fail(f"{name}: 1.0 should be the built-in payload: {ev}")
        time.sleep(3)
        screenshot(d, f"sample-{name}-1.0")
        d.wait(lambda e: e.get("event") == "live_swapped", f"{name} live swap to 2.0", 300)
        d.event("committed", timeout=120)
        time.sleep(3)
        screenshot(d, f"sample-{name}-2.0")
        results.append({"sample": name, "detail": "built-in 1.0 ran; 2.0 downloaded, swapped in live, committed"})
    finally:
        srv.kill()
        d.adb("reverse", "--remove", f"tcp:{port}", check=False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=list(SAMPLES))
    ap.add_argument("--api", default="34")
    ap.add_argument("--device")
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()
    dt.CONTAINER = not args.device
    dt.run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=dt.REPO)
    d = dt.Device(args.api, 4, existing=args.device)
    results, failures = [], []
    try:
        d.start()
        for name, pkg in SAMPLES.items():
            if args.only and name != args.only:
                continue
            try:
                sample(d, name, pkg, results)
                dt.log(f"[{name}] OK")
            except Exception as e:  # keep going with the other samples
                failures.append({"sample": name, "error": str(e)})
                dt.log(f"[{name}] FAILED: {e}")
    finally:
        d.stop(keep=args.keep)
    with open(os.path.join(EVIDENCE, "samples.json"), "w") as f:
        json.dump({"api": args.api, "results": results, "failures": failures}, f, indent=2)
    for r in results:
        dt.log(f"ok  {r['sample']}: {r['detail']}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
