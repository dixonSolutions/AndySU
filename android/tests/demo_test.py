#!/usr/bin/env python3
"""The release demo channel (scripts/demo.sh), end to end on an emulator (API 36):
two demo releases go into a local site exactly as the release workflow builds them (CI test
key, CI test keystore); the first release's APK is installed, finds the second on the channel,
swaps it in live, commits, and shows "Payload version <second>".

Needs the CI test keys in ~/.config/andysu/keys (docs/KEYS.md).
Usage: scripts/heavy.sh python3 android/tests/demo_test.py
"""
import os, sys, subprocess, json, shutil, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import device_tests as dt
import sample_tests as st
dt.CONTAINER = True
S = tempfile.mkdtemp(prefix="andysu-demo-")
K = os.path.expanduser("~/.config/andysu/keys")
env = dict(os.environ, ANDYSU_KEY_FILE=f"{K}/andysu-ci-test-root.key", ANDROID_KEYSTORE=f"{K}/andysu-ci-test.jks",
           ANDROID_KEYSTORE_PASS_FILE=f"{K}/andysu-ci-test.jks.pass", ANDROID_KEY_ALIAS="andysu-ci-test")
port = dt.free_port()
base = f"http://127.0.0.1:{port}"
for v in ["0.1.900", "0.1.901"]:
    subprocess.run(["scripts/demo.sh", f"{S}/site", base, v, f"{S}/out"], cwd=dt.REPO, env=env, check=True)
m = json.load(open(f"{S}/site/demo/stable/update.json"))
print("releases:", [r["id"] for r in m["releases"]], "deltas:", sum(len(c.get("deltas", [])) for r in m["releases"] for c in r["components"]))
v = subprocess.run([dt.ANDYSU, "verify", f"{S}/site/demo/stable/update.json", "--pub", f"{dt.REPO}/keys/andysu-ci-test-public.pem", "--blobs"], capture_output=True, text=True)
if v.returncode != 0:
    sys.exit(f"verify failed: {v.stdout}{v.stderr}")
print(v.stdout[-300:])
# the host built for 0.1.900 must still find 0.1.901: serve the site, install the 900 APK
srv = subprocess.Popen([dt.ANDYSU, "serve", f"{S}/site", "--addr", f"127.0.0.1:{port}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
d = dt.Device("36", 4)
pkg = "com.andysu.sample.compose"
try:
    d.start()
    d.adb("uninstall", pkg, check=False)
    d.install(f"{S}/out/andysu-demo-compose-0.1.900.apk")
    print(d.sh(f"dumpsys package {pkg} | grep -m1 -A1 'signatures\\|Signing'", check=False)[:200])
    d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
    d.watch()
    d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
    ev = d.wait(lambda e: e.get("event") == "booted", "booted", 120)
    print("booted", ev)
    d.wait(lambda e: e.get("event") == "live_swapped" and e.get("to") == "0.1.901", "live swap to 0.1.901", 300)
    d.event("committed", timeout=120)
    print("on screen:", st.expect_on_screen(d, "compose", "0.1.901"))
    st.screenshot(d, "demo-0.1.901")
finally:
    srv.kill(); d.stop(); shutil.rmtree(S, ignore_errors=True)
