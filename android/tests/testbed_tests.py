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

Xonotic needs its game data on disk before BootActivity starts the engine (it would otherwise
download 1.2 GB). The bed copies the pk3s from $XONOTIC_DATA_DIR (default: the Xonotic pk3 set in
~/.cache/xt-perf/base/data, read only) onto the emulator with `adb push`, enables the engine's
logcat output (+developer 1) and then requires proof that the ENGINE ran from the payload: SDL's
"Running main function SDL_main from library <slot>/libmain.so", the engine's init banner and the
GL renderer line. If the data is missing the bed fails loudly; it never passes on AndySU events
alone.

Usage: python3 android/tests/testbed_tests.py [--only xonotic|stk] [--api 34]
       python3 android/tests/testbed_tests.py --probe-vid-restart [--api 36]
         (stock Xonotic APK, no AndySU: does rendering survive `vid_restart`? see LIVE_SWAP.md)
"""
import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
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


DATA_SRC = os.path.expanduser(os.environ.get("XONOTIC_DATA_DIR", "~/.cache/xt-perf/base/data"))
REQUIRED_PK3 = ("-data.pk3", "-maps.pk3", "-music.pk3")
ENGINE_TAGS = ["SDL:V", "app_process:D", "XonoticTouch:I"]


def seed_xonotic(d, pkg, extra_args=""):
    """Put the game data where GameData expects it (<external files>/data) so BootActivity does
    not try to download it, and switch on the engine's logcat output."""
    pk3s = sorted(p for p in glob.glob(os.path.join(DATA_SRC, "*.pk3")) if os.path.isfile(p))  # follows symlinks
    for suffix in REQUIRED_PK3:
        if not any(p.endswith(suffix) for p in pk3s):
            raise dt.Fail(f"Xonotic game data missing: no *{suffix} in {DATA_SRC} (set XONOTIC_DATA_DIR). "
                          "Without it the engine cannot start, so this bed would prove nothing.")
    base = f"/sdcard/Android/data/{pkg}/files"
    # The app must create its own external directories: ones made by `adb shell` belong to the
    # shell user and the app then cannot create sub-directories in them (GameData fails with
    # "Cannot create .../data/font-dejavu.pk3dir"). So run BootActivity once without data; it
    # unpacks the slim bundle (creating files/data as the app) and stops at the download error.
    d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
    end = time.time() + 180 * dt.SLOW
    while time.time() < end:
        if d.sh(f"ls {base}/.bundle-version 2>/dev/null", check=False).strip():
            break
        time.sleep(2)
    else:
        raise dt.Fail("seeding: the app did not create its data directory (slim bundle not unpacked)")
    d.sh(f"am force-stop {pkg}")
    t0 = time.time()
    for p in pk3s:
        d.adb("push", os.path.realpath(p), f"{base}/data/{os.path.basename(p)}", timeout=1200)
    log(f"seeded {len(pk3s)} pk3s ({sum(os.path.getsize(p) for p in pk3s) >> 20} MiB) in {time.time() - t0:.0f}s")
    args = os.path.join(tempfile.mkdtemp(prefix="xonargs-"), "xonotic.args")
    with open(args, "w") as f:
        f.write("+developer 1 " + extra_args + "\n")
    d.adb("push", args, f"{base}/xonotic.args")
    return base


class EngineLog:
    """logcat of the engine side: SDL's own lines and the engine's console (developer 1)."""

    def __init__(self, d):
        d.adb("logcat", "-c")
        self.lines = []
        self.p = subprocess.Popen([dt.ADB, "-s", d.serial, "logcat", "-v", "brief", "-s", *ENGINE_TAGS],
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            self.lines.append(line.rstrip("\n"))

    def mark(self):
        return len(self.lines)

    def wait(self, pattern, what, since=0, timeout=300):
        rx = re.compile(pattern)
        end = time.time() + timeout * dt.SLOW
        seen = since
        while time.time() < end:
            while seen < len(self.lines):
                line = self.lines[seen]
                seen += 1
                if rx.search(line):
                    return line
            time.sleep(0.5)
        raise dt.Fail(f"timed out waiting for {what} (/{pattern}/)\nlast engine log lines:\n" + "\n".join(self.lines[-25:]))

    def stop(self):
        self.p.kill()

    def save(self, name):
        os.makedirs(EVIDENCE, exist_ok=True)
        with open(os.path.join(EVIDENCE, name), "w") as f:
            f.write("\n".join(self.lines) + "\n")


def engine_ran(el, since, slot_hint, what):
    """The engine really started from the payload: SDL_main ran from libmain.so inside the AndySU
    slot, the engine initialised, and GL came up. Returns the evidence lines."""
    main = el.wait(r"Running main function SDL_main from library .*libmain\.so", f"{what}: SDL_main start", since)
    if "/andysu/" not in main or slot_hint not in main:
        raise dt.Fail(f"{what}: SDL_main did not run from the payload ({slot_hint}): {main}")
    init = el.wait(r"========Initialized=========", f"{what}: engine init banner", since)
    gl = el.wait(r"GL_RENDERER: ", f"{what}: GL renderer line", since)
    log(f"[engine] {what}: {main.strip()} | {init.strip()} | {gl.strip()}")
    return [main, init, gl]


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
    ks = ["--keystore", dt.debug_keystore(), "--ks-pass", "pass:android",
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
        # No internet for the emulator: the games' own first-run downloads (Xonotic fetches
        # 1.16 GB of pk3s) are not part of this test. The remote is reached over adb reverse.
        d.sh("svc wifi disable", check=False)
        d.sh("svc data disable", check=False)
        d.install(ra["host"]["apk"])
        if name == "xonotic":
            # Before the remote is reachable (no adb reverse yet): the seeding launch must not
            # stage the update, or the next launch would already run B.
            seed_xonotic(d, pkg)
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        d.watch()
        el = EngineLog(d) if name == "xonotic" else None

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
        engine_note = ""
        if el:
            ev_lines = engine_ran(el, 0, "/factory/", f"{va} built-in")
            engine_note = "; engine ran (SDL_main from the payload's libmain.so, init banner, GL renderer)"
        time.sleep(15)
        screenshot(d, f"{name}-1-builtin-{va}")
        results.append({"bed": name, "step": "builtin", "detail": f"{va} runs from the payload built into the host APK" + engine_note})
        st = d.status("staged", timeout=900)
        log(f"[{name}] {vb} staged: {st['status'].get('download_bytes')} bytes")
        d.sh(f"am force-stop {pkg}")
        mark = el.mark() if el else 0
        launch()
        ev = booted(vb)
        if not ev.get("trial"):
            raise dt.Fail(f"{vb} should boot in trial: {ev}")
        if el:
            engine_ran(el, mark, f"/slots/{vb}/", f"{vb} updated")
        d.event("committed", timeout=240)
        time.sleep(10)
        screenshot(d, f"{name}-2-updated-{vb}")
        host_ver = installed_version()
        results.append({"bed": name, "step": "update",
                        "detail": f"{vb} applied on next start and committed; installed APK still versionName {host_ver}"
                                  + ("; engine ran from slots/" + vb if el else "")})
        if host_ver != va:
            raise dt.Fail(f"the host APK changed: versionName {host_ver}, expected {va}")

        # Broken release: rolled back after one relaunch.
        dt.run([dt.ANDYSU, "pack", broken, "--site", site, "--no-rename"], timeout=3600)
        dt.run([dt.ANDYSU, "publish", "--site", site, "--release", vb + "-broken", "--key", key])
        d.sh(f"am force-stop {pkg}")
        launch()
        booted(vb)
        d.status("staged", timeout=900)
        d.sh(f"am force-stop {pkg}")
        inits = sum(1 for e in d.events if e.get("event") == "init")
        mark = el.mark() if el else 0
        launch()
        booted(vb + "-broken")
        # The crash handler rolls back, then relaunches once; that launch runs B again.
        d.wait(lambda e: e.get("event") == "relaunch", "relaunch after the broken start", 180)
        launch_ev = d.wait(lambda e: e.get("event") == "launch", "launch after relaunch", 180)
        if (launch_ev.get("slot") or {}).get("id") != vb or launch_ev.get("in_trial"):
            raise dt.Fail(f"expected a committed {vb} after the rollback: {launch_ev}")
        booted(vb)
        if el:
            engine_ran(el, mark, f"/slots/{vb}/", f"{vb} after rollback")
        relaunches = sum(1 for e in d.events if e.get("event") == "init") - inits - 1
        if relaunches != 1:
            raise dt.Fail(f"expected exactly one relaunch after the broken start, saw {relaunches}")
        time.sleep(10)
        screenshot(d, f"{name}-3-rolled-back-{vb}")
        st = d.state() if False else None
        results.append({"bed": name, "step": "rollback",
                        "detail": f"{vb}-broken failed on start; rolled back by the crash handler; one relaunch; back on {vb}"})
    finally:
        srv.kill()
        if el:
            el.stop()
            el.save(f"{name}-engine-log.txt")
        with open(os.path.join(EVIDENCE, f"{name}-events.jsonl"), "w") as f:
            for e in d.events:
                f.write(json.dumps(e) + "\n")


def probe_vid_restart(args):
    """R1 (LIVE_SWAP.md): stock Xonotic APK, no AndySU. Reach the menu, run vid_restart, and look
    for evidence that rendering survives it. Evidence goes to docs/evidence/probe-vid-restart-*."""
    spec = BEDS["xonotic"]
    pkg = spec["pkg"]
    d = dt.Device(args.api, 3, existing=args.device)
    res = {"api": args.api}
    try:
        d.start()
        d.adb("uninstall", pkg, check=False)
        d.sh("svc wifi disable", check=False)
        d.sh("svc data disable", check=False)
        d.install(os.path.join(TB, spec["a"]))
        base = seed_xonotic(d, pkg, "+exec probe.cfg")
        cfg = os.path.join(tempfile.mkdtemp(prefix="probe-"), "probe.cfg")
        with open(cfg, "w") as f:
            f.write('echo ANDYSU_PROBE_BEGIN\n'
                    'defer 25 "echo ANDYSU_PROBE_PRE; screenshot probe-pre.png"\n'
                    'defer 35 "echo ANDYSU_PROBE_RESTART; vid_restart"\n'
                    'defer 55 "echo ANDYSU_PROBE_POST; screenshot probe-post.png"\n'
                    'defer 65 "echo ANDYSU_PROBE_DONE"\n')
        d.adb("push", cfg, f"{base}/data/probe.cfg")
        el = EngineLog(d)
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
        el.wait(r"========Initialized=========", "stock engine init", 0, timeout=600)
        el.wait(r"GL_RENDERER: ", "GL renderer", 0)
        el.wait(r"ANDYSU_PROBE_PRE", "probe pre marker", 0, timeout=300)
        el.wait(r"ANDYSU_PROBE_RESTART", "probe restart marker", 0, timeout=300)
        i_restart = next(i for i, l in enumerate(el.lines) if "ANDYSU_PROBE_RESTART" in l)
        el.wait(r"ANDYSU_PROBE_POST", "probe post marker (engine alive after vid_restart)", i_restart, timeout=300)
        el.wait(r"ANDYSU_PROBE_DONE", "probe done marker", i_restart, timeout=300)
        after = el.lines[i_restart:]
        res["vid_restart_log"] = [l for l in after if re.search(r"VID_Restart|GL_RENDERER|Display \d|Video mode|Failed|rror", l)][:20]
        res["gl_renderer_lines_total"] = sum(1 for l in el.lines if "GL_RENDERER:" in l)
        res["pid_alive"] = bool(d.sh(f"pidof {pkg}", check=False).strip())
        os.makedirs(EVIDENCE, exist_ok=True)
        for shot in ("probe-pre", "probe-post"):
            out = d.sh(f"find {base} -name '{shot}.png'", check=False).split()
            res[shot + "_files"] = out
            for fn in out:
                dest = os.path.join(EVIDENCE, "probe-vid-restart-" + shot + ".png")
                d.adb("pull", fn, dest, check=False)
                res.setdefault("pulled", []).append(dest)
        screenshot(d, "probe-vid-restart-screencap")
        el.stop()
        el.save("probe-vid-restart-engine-log.txt")
        # Evidence: the engine's own screenshot (glReadPixels of the frame it rendered) after the
        # restart exists, GL was set up a second time, and the process is the same one.
        ok = res["pid_alive"] and bool(res.get("probe-post_files")) and res["gl_renderer_lines_total"] >= 2
        res["verdict"] = "rendering survives vid_restart" if ok else "NOT PROVEN (see fields)"
        log("probe result: " + json.dumps(res, indent=2))
        with open(os.path.join(EVIDENCE, "probe-vid-restart.json"), "w") as f:
            json.dump(res, f, indent=2)
    finally:
        if not args.keep:
            d.stop()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=list(BEDS))
    ap.add_argument("--api", default=None, help="default 34 (36 for the vid_restart probe: screenshots)")
    ap.add_argument("--device")
    ap.add_argument("--keep", action="store_true", help="keep the emulator running and the work dir")
    ap.add_argument("--probe-vid-restart", action="store_true", help="R1 probe on the stock Xonotic APK (no AndySU)")
    args = ap.parse_args()
    dt.CONTAINER = not args.device
    if args.probe_vid_restart:
        args.api = args.api or "36"
        probe_vid_restart(args)
        return
    args.api = args.api or "34"
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
        if not args.keep:
            d.stop()
            shutil.rmtree(work, ignore_errors=True)
        else:
            log(f"kept: emulator {d.serial}, work dir {work}")
    os.makedirs(EVIDENCE, exist_ok=True)
    with open(os.path.join(EVIDENCE, "testbeds.json"), "w") as f:
        json.dump({"results": results, "failures": failures}, f, indent=2)
    for r in results:
        log(f"ok  {r['bed']} {r['step']}: {r['detail']}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
