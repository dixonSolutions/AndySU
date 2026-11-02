#!/usr/bin/env python3
"""Step 7: real apps (Xonotic Touch, SuperTuxKart Touch) converted with `andysu split --transparent`.

For each test bed, from its published x86_64 release APKs (testbeds/apks/, downloaded from the
projects' GitHub releases; nothing is rebuilt and no source is changed):

  1. split version A into a frozen host APK with A built in; pack version B as a payload
  2. on an emulator: A runs (engine .so, Java/SDL code and data all from the payload)
  3. B is downloaded and applied on the next start, the host APK is unchanged (versionName of
     the installed package stays A), B is committed after it ran
  4. a broken release (B with a truncated classes.dex) is published: it fails on start, the app
     relaunches once and is back on B
Screenshots and the event log go to docs/evidence/.

Usage: python3 android/tests/testbed_tests.py [--only xonotic|stk] [--api 34]
"""
import argparse
import json
import os
import shutil
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import device_tests as dt  # noqa: E402

TB = os.path.join(dt.REPO, "testbeds", "apks")
EVIDENCE = os.path.join(dt.REPO, "docs", "evidence")
BEDS = {
    "xonotic": {"pkg": "io.github.dixonsolutions.xonotictouch",
                "a": "XonoticTouch-1.2.104-x86_64.apk", "b": "XonoticTouch-1.2.105-x86_64.apk"},
    "stk": {"pkg": "io.github.dixonsolutions.supertuxkarttouch",
            "a": "SuperTuxKartTouch-1.0.29-x86_64.apk", "b": "SuperTuxKartTouch-1.0.31-x86_64.apk"},
}


def log(m):
    dt.log(m)


def screenshot(d, name):
    os.makedirs(EVIDENCE, exist_ok=True)
    p = os.path.join(EVIDENCE, name + ".png")
    import subprocess
    with open(p, "wb") as f:
        subprocess.run([dt.ADB, "-s", d.serial, "exec-out", "screencap", "-p"], stdout=f, timeout=60)
    log(f"screenshot {p}")


def bed(d, name, spec, work, results):
    pkg = spec["pkg"]
    w = os.path.join(work, name)
    os.makedirs(w, exist_ok=True)
    keys = os.path.join(w, "keys")
    os.makedirs(keys)
    dt.run([dt.ANDYSU, "keygen", "--dir", keys])
    key = os.path.join(keys, "andysu-root.key")
    site = os.path.join(w, "site")
    os.makedirs(os.path.join(site, "stable"))
    port = dt.free_port()
    cfg = {"app": pkg, "channel": "stable", "roots": [open(os.path.join(keys, "andysu-public.pem")).read()],
           "manifest_url": f"http://127.0.0.1:{port}/stable/update.json", "mode": "auto", "apply": "next_start",
           "check_on_open": True, "watchdog_ms": 10000, "auto_healthy_ms": 8000, "max_boot_attempts": 2, "host_api": 1}
    cp = os.path.join(w, "config.json")
    json.dump(cfg, open(cp, "w"))
    tpl = os.path.join(dt.ANDROID, "host-template/build/andysu/andysu-host-template.zip")
    ks = ["--keystore", os.path.expanduser("~/.android/debug.keystore"), "--ks-pass", "pass:android",
          "--key-alias", "androiddebugkey"]
    log(f"[{name}] splitting {spec['a']} (host + built-in payload) and {spec['b']} (update)")
    ra = json.loads(dt.run([dt.ANDYSU, "split", os.path.join(TB, spec["a"]), "--out", os.path.join(w, "a"), "--transparent",
                            "--host-template", tpl, "--config", cp, "--cleartext", *ks], timeout=3600))
    rb = json.loads(dt.run([dt.ANDYSU, "split", os.path.join(TB, spec["b"]), "--out", os.path.join(w, "b"), "--transparent"],
                           timeout=3600))
    va, vb = ra["version"], rb["version"]
    for wn in ra["host"]["warnings"]:
        log(f"[{name}] split warning: {wn}")
    t0 = time.time()
    dt.run([dt.ANDYSU, "pack", os.path.join(w, "b", "payload"), "--site", site, "--no-rename"], timeout=3600)
    dt.run([dt.ANDYSU, "publish", "--site", site, "--release", vb, "--key", key])
    log(f"[{name}] packed and published {vb} in {time.time() - t0:.0f}s")
    # The broken release: B with a truncated classes.dex.
    broken = os.path.join(w, "broken")
    shutil.copytree(os.path.join(w, "b", "payload"), broken)
    dex = os.path.join(broken, "dex", "classes.dex")
    data = open(dex, "rb").read()
    open(dex, "wb").write(data[: len(data) // 3])
    p = os.path.join(broken, "andysu-payload.json")
    desc = json.load(open(p))
    desc["version"] = vb + "-broken"
    json.dump(desc, open(p, "w"))

    import subprocess
    srv = subprocess.Popen([dt.ANDYSU, "serve", site, "--addr", f"127.0.0.1:{port}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
    try:
        d.adb("uninstall", pkg, check=False)
        d.install(ra["host"]["apk"])
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        d.watch()

        def launch():
            d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)

        def booted(version, timeout=240):
            return d.wait(lambda e: e.get("event") == "booted" and e.get("version") == version, f"{version} booted",
                          timeout)

        def installed_version():
            out = d.sh(f"dumpsys package {pkg} | grep versionName", check=False)
            return out.strip().split("=")[-1] if out.strip() else "?"

        launch()
        ev = booted(va)
        if not (ev.get("transparent") and ev.get("factory")):
            raise dt.Fail(f"{va} should boot transparently from the built-in payload: {ev}")
        time.sleep(15)
        screenshot(d, f"{name}-1-builtin-{va}")
        results.append({"bed": name, "step": "builtin", "detail": f"{va} runs from the payload built into the host APK"})
        st = d.status("staged", timeout=900)
        log(f"[{name}] {vb} staged: {st['status'].get('download_bytes')} bytes")
        d.force_stop()
        launch()
        ev = booted(vb)
        if not ev.get("trial"):
            raise dt.Fail(f"{vb} should boot in trial: {ev}")
        d.event("committed", timeout=240)
        time.sleep(10)
        screenshot(d, f"{name}-2-updated-{vb}")
        host_ver = installed_version()
        results.append({"bed": name, "step": "update",
                        "detail": f"{vb} applied on next start and committed; installed APK still versionName {host_ver}"})
        if host_ver != va:
            raise dt.Fail(f"the host APK changed: versionName {host_ver}, expected {va}")

        # Broken release: rolled back after one relaunch.
        dt.run([dt.ANDYSU, "pack", broken, "--site", site, "--no-rename"], timeout=3600)
        dt.run([dt.ANDYSU, "publish", "--site", site, "--release", vb + "-broken", "--key", key])
        d.force_stop()
        launch()
        booted(vb)
        d.status("staged", timeout=900)
        d.force_stop()
        inits = sum(1 for e in d.events if e.get("event") == "init")
        launch()
        booted(vb + "-broken")
        launch_ev = d.wait(lambda e: e.get("event") == "launch" and any(
            x.get("event") == "rolled_back" for x in e.get("events", [])), "rollback on relaunch", 180)
        booted(vb)
        relaunches = sum(1 for e in d.events if e.get("event") == "init") - inits
        time.sleep(10)
        screenshot(d, f"{name}-3-rolled-back-{vb}")
        st = d.state() if False else None
        results.append({"bed": name, "step": "rollback",
                        "detail": f"{vb}-broken failed on start; {relaunches - 1} relaunch; back on {vb}"})
    finally:
        srv.kill()
        with open(os.path.join(EVIDENCE, f"{name}-events.jsonl"), "w") as f:
            for e in d.events:
                f.write(json.dumps(e) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=list(BEDS))
    ap.add_argument("--api", default="34")
    ap.add_argument("--device")
    args = ap.parse_args()
    dt.CONTAINER = not args.device
    work = tempfile.mkdtemp(prefix="andysu-testbeds-")
    dt.run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=dt.REPO)
    dt.gradle(":host-template:andysuHostTemplate")
    d = dt.Device(args.api, 3, existing=args.device)
    results, failures = [], []
    try:
        d.start()
        for name, spec in BEDS.items():
            if args.only and name != args.only:
                continue
            try:
                bed(d, name, spec, work, results)
                log(f"[{name}] OK")
            except Exception as e:
                failures.append({"bed": name, "error": str(e), "diagnostics": d.diagnostics()})
                log(f"[{name}] FAILED: {e}\n{failures[-1]['diagnostics'][-3000:]}")
    finally:
        d.stop()
        shutil.rmtree(work, ignore_errors=True)
    os.makedirs(EVIDENCE, exist_ok=True)
    with open(os.path.join(EVIDENCE, "testbeds.json"), "w") as f:
        json.dump({"results": results, "failures": failures}, f, indent=2)
    for r in results:
        log(f"ok  {r['bed']} {r['step']}: {r['detail']}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
