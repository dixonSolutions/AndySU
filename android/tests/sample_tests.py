#!/usr/bin/env python3
"""The samples (Compose, native GLES, WebView) on an emulator, each built by samples/build.sh:

  1. the host APK starts its built-in payload 1.0 (with nothing but normal permissions)
  2. it finds 2.0 on the remote, applies it live (apply = "live") and commits it
  3. what is on screen is checked through the accessibility tree (uiautomator: the Compose and
     WebView samples show "Payload version <v>"), and a screenshot of each version goes to
     docs/evidence/. ATD images (aosp_atd) capture normal windows as black frames, so the default
     image is API 36 google_apis.

Usage: python3 android/tests/sample_tests.py [--only compose|native|webview] [--api 36] [--device SERIAL]
"""
import argparse
import json
import os
import re
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


def screen_texts(d):
    """(text, top px) of every labelled node, and how far down the window is covered by the
    status bar or an action bar."""
    d.sh("uiautomator dump /sdcard/andysu-ui.xml", check=False, timeout=60)
    xml = d.sh("cat /sdcard/andysu-ui.xml", check=False)
    texts = [(t, int(top)) for t, top in re.findall(r'text="([^"]+)"[^>]*?bounds="\[\d+,(\d+)\]', xml)]
    covered = [int(b) for b in re.findall(
        r'resource-id="android:id/action_bar_container"[^>]*?bounds="\[\d+,\d+\]\[\d+,(\d+)\]', xml)]
    bars = d.sh("dumpsys window | grep 'type=statusBars'", check=False)
    covered += [int(t) for t in re.findall(r"insetsSize=Insets\{left=0, top=(\d+)", bars)]
    return texts, max(covered, default=0)


def expect_on_screen(d, name, version, timeout=30):
    """The UI samples print their payload version; the native one draws with GL (no text)."""
    if name == "native":
        return "GL frame (no text)"
    end = time.time() + timeout * dt.SLOW
    texts = []
    while time.time() < end:
        texts, covered = screen_texts(d)
        hit = [(t, top) for t, top in texts if f"version {version}" in t]
        if hit and hit[0][1] >= covered:
            return hit[0][0]
        if hit:
            # In the tree but drawn under the status bar or an action bar (edge to edge, 15+).
            raise dt.Fail(f"{name}: {hit[0][0]!r} at y={hit[0][1]} is covered (system/action bar to y={covered})")
        time.sleep(2)
    raise dt.Fail(f"{name}: 'version {version}' not on screen; saw {texts}")


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
        # First run without the remote, so 1.0 can be seen before the update swaps it out.
        d.watch()
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
        ev = d.wait(lambda e: e.get("event") == "booted" and e.get("version") == "1.0", f"{name} 1.0 booted", 120)
        if not ev.get("factory"):
            raise dt.Fail(f"{name}: 1.0 should be the built-in payload: {ev}")
        shown1 = expect_on_screen(d, name, "1.0")
        screenshot(d, f"sample-{name}-1.0")
        # Now the remote: the next start checks on open and swaps 2.0 in live.
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        d.sh(f"am force-stop {pkg}")
        d.watch()
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
        d.wait(lambda e: e.get("event") == "live_swapped", f"{name} live swap to 2.0", 300)
        d.event("committed", timeout=120)
        shown2 = expect_on_screen(d, name, "2.0")
        screenshot(d, f"sample-{name}-2.0")
        results.append({"sample": name, "detail": f"built-in 1.0 ran ({shown1!r}); 2.0 downloaded, swapped in "
                                                  f"live, committed ({shown2!r})"})
    finally:
        srv.kill()
        d.adb("reverse", "--remove", f"tcp:{port}", check=False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=list(SAMPLES))
    ap.add_argument("--api", default="36")
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
