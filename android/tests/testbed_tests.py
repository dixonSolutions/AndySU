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
       python3 android/tests/testbed_tests.py --low-storage --apk-a A --apk-b B --apk-c C --apk-d D --apk-e E
         (GC guard under faked low storage; see low_storage_bed)
       python3 android/tests/testbed_tests.py --probe-vid-restart [--api 36]
         (stock Xonotic APK, no AndySU: does rendering survive `vid_restart`? see LIVE_SWAP.md)
"""
import argparse
import atexit
import glob
import json
import os
import queue
import re
import shutil
import struct
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
TEMP_DIRS = []
KEEP_TEMP_DIRS = False


def temp_dir(prefix):
    path = tempfile.mkdtemp(prefix=prefix)
    TEMP_DIRS.append(path)
    return path


def cleanup_temp_dirs():
    if not KEEP_TEMP_DIRS:
        for path in reversed(TEMP_DIRS):
            shutil.rmtree(path, ignore_errors=True)


def stop_process(proc, timeout=5):
    """Stop and reap a test-owned subprocess, including any captured output pipe."""
    if proc is None:
        return
    if proc.poll() is None:
        try:
            proc.terminate()
        except ProcessLookupError:
            pass
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            try:
                proc.kill()
            except ProcessLookupError:
                pass
            proc.wait(timeout=timeout)
    else:
        proc.wait()
    if proc.stdout and not proc.stdout.closed:
        proc.stdout.close()


def stop_device(d):
    """Use the shared Device cleanup, then reap the test's logcat/emulator children."""
    try:
        d.stop()
    finally:
        stop_process(d.logcat)
        if d.emu:
            if d.emu.poll() is None:
                try:
                    d.emu.kill()
                except ProcessLookupError:
                    pass
            try:
                d.emu.wait(timeout=5)
            except subprocess.TimeoutExpired:
                log(f"warning: emulator runner {d.emu.pid} did not exit after kill")
LOADER = False  # set by --apk-a/--apk-b: APKs built with patch 0002 start the engine through the loader
ENGINE_TAGS = ["SDL:V", "app_process:D", "XonoticTouch:I", "AndySU-Event:I"]


FIRST_RUN_SKIP = ("+set touch_profile_done 1 +set touch_setup_done 1 +set _termsofservice_accepted 2147483647 "
                  "+set _cl_name AndySU +set cl_allow_uid2name 0")


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
    arg_dir = temp_dir("xonargs-")
    args = os.path.join(arg_dir, "xonotic.args")
    with open(args, "w") as f:
        # Skip first-run dialogs so every start lands on the same menu for screen_alive.
        f.write("+developer 1 " + FIRST_RUN_SKIP + " " + extra_args + "\n")
    d.adb("push", args, f"{base}/xonotic.args")
    return base


class EngineLog:
    """logcat of the engine side: SDL's own lines and the engine's console (developer 1)."""

    def __init__(self, d):
        d.adb("logcat", "-c")
        self.lines = []
        self.p = subprocess.Popen([dt.ADB, "-s", d.serial, "logcat", "-v", "brief", "-s", *ENGINE_TAGS],
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.reader = threading.Thread(target=self._read, name="andysu-engine-logcat")
        self.reader.start()

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
        stop_process(self.p)
        self.reader.join(timeout=5)
        if self.reader.is_alive():
            raise dt.Fail("engine logcat reader did not stop after its pipe closed")

    def save(self, name):
        os.makedirs(EVIDENCE, exist_ok=True)
        with open(os.path.join(EVIDENCE, name), "w") as f:
            f.write("\n".join(self.lines) + "\n")


def engine_ran(el, since, slot_hint, what, loader=False):
    """The engine really started from the payload: SDL_main ran from libmain.so inside the AndySU
    slot, the engine initialised, and GL came up. Returns the evidence lines.

    loader=True (APKs built with patch 0002): SDL runs the resident loader's AndySU_SDL_main from
    libandysu_xonotic_loader.so, the loader reports loading libmain from the slot, and the engine's
    entry (andysu_engine_v1 run) reports itself, before the usual init banner and GL line."""
    if loader:
        main = el.wait(r"Running main function AndySU_SDL_main from library .*libandysu_xonotic_loader\.so",
                       f"{what}: loader SDL main", since)
        loaded = el.wait(r'"event":"loader_engine_loaded".*libmain[^"]*\.so', f"{what}: loader dlopen of libmain", since)
        entry = el.wait(r'"event":"engine_entry"', f"{what}: engine entry (andysu_engine_v1 run)", since)
        if "/andysu/" not in main or slot_hint not in main or slot_hint not in loaded:
            raise dt.Fail(f"{what}: loader/engine did not come from the payload ({slot_hint}): {main} / {loaded}")
    else:
        main = el.wait(r"Running main function SDL_main from library .*libmain\.so", f"{what}: SDL_main start", since)
        loaded = entry = ""
        if "/andysu/" not in main or slot_hint not in main:
            raise dt.Fail(f"{what}: SDL_main did not run from the payload ({slot_hint}): {main}")
    init = el.wait(r"========Initialized=========", f"{what}: engine init banner", since)
    gl = el.wait(r"GL_RENDERER: ", f"{what}: GL renderer line", since)
    log(f"[engine] {what}: {main.strip()} | {loaded.strip()} | {entry.strip()} | {init.strip()} | {gl.strip()}")
    return [main, loaded, entry, init, gl]


def screenshot(d, name):
    os.makedirs(EVIDENCE, exist_ok=True)
    p = os.path.join(EVIDENCE, name + ".png")
    import subprocess
    with open(p, "wb") as f:
        subprocess.run([dt.ADB, "-s", d.serial, "exec-out", "screencap", "-p"],
                       stdout=f, stderr=subprocess.DEVNULL, timeout=60, check=True)
    log(f"screenshot {p}")


def no_immersive_prompt(d):
    """Android shows a "Viewing full screen" bubble the first time an app goes immersive; it covers the game and
    takes key presses (screen_alive would see a frozen screen). Mark it confirmed on our own emulators."""
    if not d.existing:
        d.sh("settings put secure immersive_mode_confirmations confirmed", check=False)


def screen_alive(d, what):
    """Verify the screen is actually rendering: capture frame, send key, retry up to 3 times to catch
    screen movement. A frozen frame has no pixel change. Require sampled grayscale standard deviation >=10
    in both frames and at least 2% sampled pixels to change. Uses raw screencap bytes only."""

    def capture_screencap():
        """Capture raw screencap binary (not -p). Returns raw binary data."""
        try:
            result = subprocess.run([dt.ADB, "-s", d.serial, "exec-out", "screencap"],
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=60, check=True)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as e:
            raise dt.Fail(f"screen_alive: screencap command failed: {e}") from e
        return result.stdout

    def parse_screencap(data):
        """Parse raw Android screencap format: header (12 or 16 bytes) + RGBA pixel data."""
        if len(data) < 12:
            raise ValueError(f"screencap too short ({len(data)} bytes)")
        w = struct.unpack('<I', data[0:4])[0]
        h = struct.unpack('<I', data[4:8])[0]
        expected_pixel_size = w * h * 4
        header_size = len(data) - expected_pixel_size
        if header_size not in (12, 16):
            raise ValueError(f"screencap header size {header_size} not 12 or 16")
        fmt = struct.unpack('<I', data[8:12])[0]
        if fmt != 1:
            raise ValueError(f"screencap format {fmt} is not 1 (RGBA_8888)")
        rgba_data = data[header_size:header_size+expected_pixel_size]
        return w, h, rgba_data

    def sampled_stats(w, h, rgba_data):
        """Compute pixel std-dev on a sampled grid (every 8th pixel in both directions)."""
        if not rgba_data or len(rgba_data) < w*h*4:
            return 0.0
        values = []
        for y in range(0, h, 8):
            for x in range(0, w, 8):
                idx = (y * w + x) * 4
                if idx + 4 <= len(rgba_data):
                    r, g, b, a = struct.unpack('BBBB', rgba_data[idx:idx+4])
                    values.append((r + g + b) // 3)
        if not values:
            return 0.0
        mean = sum(values) / len(values)
        var = sum((v - mean)**2 for v in values) / len(values)
        return var ** 0.5

    def changed_pixel_fraction(w, h, rgba1, rgba2):
        """Measure fraction of pixels that changed between two frames (sampled grid)."""
        if not rgba1 or not rgba2 or len(rgba1) < w*h*4 or len(rgba2) < w*h*4:
            return 0.0
        changed = 0
        total = 0
        for y in range(0, h, 8):
            for x in range(0, w, 8):
                idx = (y * w + x) * 4
                if idx + 4 <= min(len(rgba1), len(rgba2)):
                    r1, g1, b1, a1 = struct.unpack('BBBB', rgba1[idx:idx+4])
                    r2, g2, b2, a2 = struct.unpack('BBBB', rgba2[idx:idx+4])
                    if abs(int(r1) - int(r2)) + abs(int(g1) - int(g2)) + abs(int(b1) - int(b2)) > 20:
                        changed += 1
                    total += 1
        return changed / total if total > 0 else 0.0

    os.makedirs(EVIDENCE, exist_ok=True)

    # Capture reference frame
    cap1 = capture_screencap()
    try:
        w, h, rgba1 = parse_screencap(cap1)
    except Exception as e:
        raise dt.Fail(f"screen_alive: failed to parse first frame: {e}")
    std1 = sampled_stats(w, h, rgba1)
    std2 = 0.0
    changed_frac = 0.0

    # Retry up to 3 times: key, wait 1.5s, capture; pass as soon as one attempt shows >= 2% change
    UNIFORM_THRESHOLD = 10
    CHANGE_THRESHOLD = 0.02  # 2%: between measured 9-13% (healthy) and ~0% (frozen)
    max_attempts = 3

    # Something only a live, drawing engine shows: first the console (` toggles it over the top half of
    # the screen; it is closed again below), then menu navigation. A frozen frame shows none of them.
    keys = ["KEYCODE_GRAVE", "KEYCODE_DPAD_DOWN", "KEYCODE_DPAD_UP"]
    for attempt in range(max_attempts):
        key = keys[attempt % len(keys)]
        d.sh(f"input keyevent {key}", check=False)
        time.sleep(1.5)
        cap2 = capture_screencap()
        if key == "KEYCODE_GRAVE":
            d.sh("input keyevent KEYCODE_GRAVE", check=False)  # close the console again
            time.sleep(0.5)
        try:
            w2, h2, rgba2 = parse_screencap(cap2)
        except Exception as e:
            if attempt == max_attempts - 1:
                raise dt.Fail(f"screen_alive: failed to parse frame on attempt {attempt+1}: {e}")
            continue
        if (w2, h2) != (w, h):
            changed_frac = 0.0
            log(f"[screen] {what}: attempt {attempt+1} dimensions changed from {w}x{h} to {w2}x{h2}; retrying")
            continue
        std2 = sampled_stats(w2, h2, rgba2)
        changed_frac = changed_pixel_fraction(w, h, rgba1, rgba2)

        # Check: both frames must be non-uniform AND show movement
        if std1 >= UNIFORM_THRESHOLD and std2 >= UNIFORM_THRESHOLD and changed_frac >= CHANGE_THRESHOLD:
            log(f"[screen] {what}: alive (std {std1:.1f}/{std2:.1f}, change {changed_frac:.1%}, {w}x{h}) [attempt {attempt+1}, {key}]")
            return

        # Log failed attempt for diagnostics
        log(f"[screen] {what}: attempt {attempt+1} ({key}): std {std1:.1f}/{std2:.1f}, change {changed_frac:.1%} - retrying...")

    # All attempts failed: save what is on screen now as evidence (PNG, viewable)
    p1 = os.path.join(EVIDENCE, f"screen-alive-fail-{what}.png")
    with open(p1, "wb") as f:
        try:
            subprocess.run([dt.ADB, "-s", d.serial, "exec-out", "screencap", "-p"],
                           stdout=f, stderr=subprocess.DEVNULL, timeout=60, check=True)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired, OSError) as e:
            raise dt.Fail(f"screen_alive: failed to save failure screenshot: {e}") from e
    msg = (f"screen not alive ({what}): std {std1:.1f}/{std2:.1f}, but only {changed_frac:.1%} of pixels changed "
           f"after {max_attempts} key presses (need {CHANGE_THRESHOLD:.0%}): frozen\n  saved {p1}")
    raise dt.Fail(msg)


def wait_committed(d, name, timeout=240):
    """Wait for the trial to commit. A game's own self-updater may open Android's "install unknown apps"
    screen over it (seen on a BlissOS VM, whose network stays on); the game then never resumes and the
    host's auto-healthy cannot fire. Press BACK once per appearance of exactly that Settings activity."""
    end = time.time() + timeout
    front = False
    while time.time() < end:
        try:
            ev = d.q.get(timeout=2)
            if ev.get("event") == "committed":
                return ev
        except queue.Empty:
            pass
        focus = d.sh("dumpsys window | grep mCurrentFocus", check=False)
        now = "com.android.settings" in focus and "ManageAppExternalSourcesActivity" in focus
        if now and not front:
            log(f"[{name}] the game's updater opened the unknown-sources Settings screen; pressing BACK ({focus.strip()})")
            d.sh("input keyevent KEYCODE_BACK", check=False)
        front = now
    return d.event("committed", timeout=5)  # also accepts the payload events file; raises with the event tail


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
        d.fresh(pkg)
        # No internet for the emulator: the games' own first-run downloads (Xonotic fetches
        # 1.16 GB of pk3s) are not part of this test. The remote is reached over adb reverse.
        if not d.existing:  # a shared device (--device) keeps its network: adb may run over it
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
            ev_lines = engine_ran(el, 0, "/factory/", f"{va} built-in", LOADER)
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
            engine_ran(el, mark, f"/slots/{vb}/", f"{vb} updated", LOADER)
        wait_committed(d, name)
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
            engine_ran(el, mark, f"/slots/{vb}/", f"{vb} after rollback", LOADER)
        relaunches = sum(1 for e in d.events if e.get("event") == "init") - inits - 1
        if relaunches != 1:
            raise dt.Fail(f"expected exactly one relaunch after the broken start, saw {relaunches}")
        time.sleep(10)
        screenshot(d, f"{name}-3-rolled-back-{vb}")
        st = d.state() if False else None
        results.append({"bed": name, "step": "rollback",
                        "detail": f"{vb}-broken failed on start; rolled back by the crash handler; one relaunch; back on {vb}"})
    finally:
        stop_process(srv)
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
    el = None
    try:
        d.start()
        no_immersive_prompt(d)
        d.fresh(pkg)
        if not d.existing:  # a shared device (--device) keeps its network: adb may run over it
            d.sh("svc wifi disable", check=False)
            d.sh("svc data disable", check=False)
        d.install(os.path.join(TB, spec["a"]))
        base = seed_xonotic(d, pkg, "+exec probe.cfg")
        cfg = os.path.join(temp_dir("probe-"), "probe.cfg")
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
        try:
            if el and el.reader.is_alive():
                el.stop()
                el.save("probe-vid-restart-engine-log.txt")
        finally:
            if not args.keep:
                stop_device(d)


# ----------------------------------------------------------------------------- live swap probe
# Milestone 2a (LIVE_SWAP.md): the in-process engine swap itself, with no AndySU host involved. A locally
# built APK (testbeds/patches/xonotic-touch/build.sh, patches 0002+0003) is installed as a plain app; the
# loader's TEST HOOK (a file <external files>/andysu-swap-to naming an engine .so) triggers the swaps.


def events_since(el, since):
    out = []
    for i in range(since, len(el.lines)):
        m = re.search(r"AndySU-Event\(\s*\d+\): (\{.*\})", el.lines[i])
        if m:
            try:
                out.append((i, json.loads(m.group(1))))
            except json.JSONDecodeError:
                pass
    return out


def wait_event(el, since, name, what=None, timeout=240, **match):
    """First AndySU-Event `name` (fields equal to `match`) after line index `since`. -> (index, event)"""
    end = time.time() + timeout * dt.SLOW
    while time.time() < end:
        for i, e in events_since(el, since):
            if e.get("event") == name and all(e.get(k) == v for k, v in match.items()):
                return i, e
        time.sleep(0.5)
    raise dt.Fail(f"timed out waiting for event {name} {match} {what or ''}\nlast events:\n" +
                  "\n".join(json.dumps(e)[:300] for _, e in events_since(el, 0)[-14:]))


def wait_engine_start(el, since, what, timeout=300, **match):
    """`engine_start` after `since`. liblog drops writes while logd is busy (a cold start is a burst), so if
    that line is missing but the loader's `loader_engine_loaded` and the engine's own `engine_entry` with the
    same build and swap count are there, accept `loader_engine_loaded` (same pid, lib, build_id, swapped)."""
    end = time.time() + timeout * dt.SLOW
    seen = None
    while time.time() < end:
        loaded = entry = None
        for i, e in events_since(el, since):
            if not all(e.get(k) == v for k, v in match.items() if k in ("build_id", "swapped")):
                continue
            if e.get("event") == "engine_start" and all(e.get(k) == v for k, v in match.items()):
                return e
            if e.get("event") == "loader_engine_loaded" and loaded is None:
                loaded = e
            if e.get("event") == "engine_entry" and loaded is not None:
                entry = e
        if loaded and entry:
            seen = seen or time.time()
            if time.time() - seen > 10:
                log(f"WARNING: {what}: engine_start log line missing; accepted loader_engine_loaded + engine_entry "
                    f"(pid {loaded.get('pid')}, build {loaded.get('build_id')})")
                return dict(loaded, _from="loader_engine_loaded")
        time.sleep(0.5)
    return wait_event(el, since, "engine_start", what, timeout=0, **match)[1]


def extract_lib(apk, dest, abi="x86_64"):
    import zipfile
    with zipfile.ZipFile(apk) as z, open(dest, "wb") as f:
        f.write(z.read(f"lib/{abi}/libmain.so"))
    return dest


FDS_BOUND, THREADS_BOUND, RSS_BOUND_KB = 12, 3, 120 * 1024  # growth allowed over the three swaps


def live_swap_probe(args):
    if not (args.apk_a and args.apk_b):
        raise SystemExit("--live-swap-probe needs --apk-a and --apk-b (APKs built by testbeds/patches/xonotic-touch/build.sh)")
    spec = BEDS["xonotic"]
    pkg = spec["pkg"]
    work = temp_dir("liveswap-")
    libs = {"A": extract_lib(os.path.abspath(args.apk_a), os.path.join(work, "A.so")),
            "B": extract_lib(os.path.abspath(args.apk_b), os.path.join(work, "B.so"))}
    d = dt.Device(args.api, 3, existing=args.device)
    res = {"api": args.api, "swaps": []}
    el = None
    try:
        d.start()
        no_immersive_prompt(d)
        d.fresh(pkg)
        if not d.existing:  # a shared device (--device) keeps its network: adb may run over it
            d.sh("svc wifi disable", check=False)
            d.sh("svc data disable", check=False)
        d.install(os.path.abspath(args.apk_a))
        base = seed_xonotic(d, pkg, "+exec probe.cfg" if args.map else "")
        d.sh(f"touch {base}/andysu-swap-hook")  # the loader's TEST HOOK thread only starts with this flag file
        if args.map:
            cfg = os.path.join(work, "probe.cfg")
            with open(cfg, "w") as f:
                f.write(f'defer 15 "map {args.map}"\n')
            d.adb("push", cfg, f"{base}/data/probe.cfg")
        el = EngineLog(d)
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)
        _, start0 = wait_event(el, 0, "engine_start", "first engine", timeout=300, swapped=0)
        pid = start0["pid"]
        a_id, b_id = None, None
        _, ent = wait_event(el, 0, "engine_entry", "first engine entry", swapped=0)
        a_id = ent["build_id"]
        el.wait(r"========Initialized=========", "A init banner", 0, timeout=300)
        el.wait(r"GL_RENDERER: ", "A GL renderer", 0)
        wait_event(el, 0, "engine_frames", "A rendering", timeout=300)
        if args.map:
            el.wait(r"(?i)\b" + re.escape(args.map) + r"\b", f"local game on map {args.map}", 0, timeout=300)
            time.sleep(25)
        time.sleep(12)
        base_stat = [e for _, e in events_since(el, 0) if e.get("event") == "proc_stats"][-1]
        res["steady_before"] = {k: base_stat[k] for k in ("fds", "threads", "rss_kb", "fd_kinds")}
        log(f"[probe] engine A {a_id} running, pid {pid}; steady {json.dumps(res['steady_before'])}")
        # B's build id comes from its own descriptor: read it from the first swap's begin event.
        series = [("B", False), ("A", False), ("B", False)]
        prev = a_id
        res["start"] = start0
        d.sh(f"mkdir -p {base}/swap-src", check=False)
        for n, (which, fail) in enumerate(series + [("A", True)], 1):
            mark = el.mark()
            src = f"{base}/swap-src/{which}-{n}.so"
            d.adb("push", libs[which], src, timeout=300)
            trig = os.path.join(work, "andysu-swap-to")
            with open(trig, "w") as f:
                f.write(src + "\n" + ("fail_init\n" if fail else ""))
            d.adb("push", trig, f"{base}/andysu-swap-to")
            t0 = time.time()
            _, begin = wait_event(el, mark, "engine_swap_begin", f"swap {n}", timeout=60)
            if begin["from"] != prev:
                raise dt.Fail(f"swap {n}: from {begin['from']} != running {prev}")
            _, saved = wait_event(el, mark, "engine_state_saved", f"swap {n}")
            _, stopped = wait_event(el, mark, "engine_stopped", f"swap {n}")
            _, started = wait_event(el, mark, "engine_start", f"swap {n}", swapped=1)
            row = {"n": n, "to": begin["to"], "fail_init": fail, "state": saved["state"],
                   "stopped": {k: stopped[k] for k in ("fds", "threads", "rss_kb")},
                   "started": {k: started[k] for k in ("fds", "threads", "rss_kb", "libmain_maps")}}
            if not fail:
                for e in (stopped, started):
                    if e["pid"] != pid:
                        raise dt.Fail(f"swap {n}: pid changed {pid} -> {e['pid']}")
                if started["build_id"] != begin["to"]:
                    raise dt.Fail(f"swap {n}: started {started['build_id']}, expected {begin['to']}")
                _, rest = wait_event(el, mark, "engine_state_restored", f"swap {n} state carried", build_id=begin["to"])
                if rest["state"].get("swap_count") != n:
                    raise dt.Fail(f"swap {n}: state not carried: {rest['state']}")
                if args.map and (rest["state"].get("map") != args.map or saved["state"].get("map") != args.map):
                    raise dt.Fail(f"swap {n}: map {args.map} not carried: saved {saved['state']} restored {rest['state']}")
                el.wait(r"========Initialized=========", f"swap {n}: init banner", mark, timeout=300)
                el.wait(r"GL_RENDERER: ", f"swap {n}: GL renderer", mark)
                _, healthy = wait_event(el, mark, "engine_healthy", f"swap {n} healthy", timeout=300)
                wait_event(el, mark, "engine_frames", f"swap {n} frames", build_id=begin["to"], timeout=300)
                row["shutdown_ms"] = stopped["t_ms"] - begin["t_ms"]
                row["to_healthy_ms"] = healthy["t_ms"] - begin["t_ms"]
                row["seconds"] = round(time.time() - t0, 1)
                time.sleep(11)  # settle, then a steady-state sample (loader TEST HOOK emits proc_stats every 5 s)
                steady = [e for _, e in events_since(el, mark) if e.get("event") == "proc_stats"][-1]
                row["steady"] = {k: steady[k] for k in ("fds", "threads", "rss_kb", "fd_kinds")}
                prev = begin["to"]
                b_id = b_id or (begin["to"] if which == "B" else None)
            else:
                # rollback layer 2: the new engine fails its init; the loader reloads the previous build
                _, failed = wait_event(el, mark, "engine_start_failed", "forced init failure", timeout=120)
                _, rb = wait_event(el, mark, "engine_rolled_back", "rollback", timeout=120)
                _, st2 = wait_event(el, mark, "engine_start", "restart of the previous build", swapped=2)
                if rb["build_id"] != prev or st2["build_id"] != prev or st2["pid"] != pid:
                    raise dt.Fail(f"rollback went to {rb['build_id']}, expected {prev} in pid {pid}: {rb} {st2}")
                el.wait(r"========Initialized=========", "rollback: init banner", mark, timeout=300)
                wait_event(el, mark, "engine_frames", "rollback frames", build_id=prev, timeout=300)
                row["error"] = failed["error"]
                row["seconds"] = round(time.time() - t0, 1)
                row["started"] = {k: st2[k] for k in ("fds", "threads", "rss_kb", "libmain_maps")}
            res["swaps"].append(row)
            log(f"[probe] swap {n} -> {begin['to']}{' (forced failure, rolled back)' if fail else ''}: {json.dumps(row)}")
            time.sleep(3)
        # the engine must keep running after the last swap
        mark = el.mark()
        time.sleep(30)
        frames = [e["frames"] for _, e in events_since(el, mark) if e.get("event") == "engine_frames"]
        res["frames_after_30s"] = frames
        if len(frames) < 2 or frames[-1] <= frames[0]:
            raise dt.Fail(f"engine not rendering 30 s after the last swap: frame events {frames}")
        errs = [e for _, e in events_since(el, 0) if e.get("event") in ("loader_failed", "rollback_failed", "engine_swap_refused")]
        if errs:
            raise dt.Fail(f"loader errors: {errs}")
        res["pid"] = pid
        res["pid_unchanged"] = d.sh(f"pidof {pkg}", check=False).split() == [str(pid)]
        if not res["pid_unchanged"]:
            raise dt.Fail("pidof does not show the original pid")
        screenshot(d, "live-swap-final")
        ok = [r for r in res["swaps"] if not r["fail_init"]]
        st = [res["steady_before"]] + [r["steady"] for r in ok]
        res["steady_fds"] = [x["fds"] for x in st]
        res["steady_threads"] = [x["threads"] for x in st]
        res["steady_rss_kb"] = [x["rss_kb"] for x in st]
        res["libmain_maps"] = [r["started"]["libmain_maps"] for r in res["swaps"]]
        # Bounds for three swaps (measured, see LIVE_SWAP.md section 6): the engine has to stay
        # near its steady state, not leak a file descriptor set or a thread per swap.
        if st[-1]["fds"] - st[0]["fds"] > FDS_BOUND or st[-1]["threads"] - st[0]["threads"] > THREADS_BOUND \
                or st[-1]["rss_kb"] - st[0]["rss_kb"] > RSS_BOUND_KB:
            raise dt.Fail(f"resources grew beyond the bound over 3 swaps: fds {res['steady_fds']}, threads "
                          f"{res['steady_threads']}, rss_kb {res['steady_rss_kb']}")
        res["verdict"] = "live swap works"
        log("probe result: " + json.dumps(res, indent=1))
        os.makedirs(EVIDENCE, exist_ok=True)
        with open(os.path.join(EVIDENCE, "live-swap-probe.json"), "w") as f:
            json.dump(res, f, indent=2)
    finally:
        try:
            if el:
                el.stop()
                el.save("live-swap-engine-log.txt")
        finally:
            if not args.keep:
                stop_device(d)


# ----------------------------------------------------------------------------- live bed (D29)
# Milestone 2b: AndySU drives the engine swap. Locally built APKs (build.sh): A and B differ only in
# libmain (the dex is byte-identical), F is a broken engine (init always fails), D changes the dex.

JOB_ID = 0x414e4459  # com.andysu.host.Jobs.JOB_ID: `cmd jobscheduler run -f` runs a check in the live process


def live_bed(args):
    need = {"--apk-a": args.apk_a, "--apk-b": args.apk_b, "--apk-fail": args.apk_fail, "--apk-dex": args.apk_dex}
    missing = [k for k, v in need.items() if not v]
    if missing:
        raise SystemExit("--live needs " + ", ".join(sorted(need)) + f" (missing {', '.join(missing)}); build them with "
                         "testbeds/patches/xonotic-touch/build.sh --variants \"1.2.9001 1.2.9002 1.2.9003:fail 1.2.9004:dex\"")
    pkg = BEDS["xonotic"]["pkg"]
    apk = {k: os.path.abspath(v) for k, v in {"A": args.apk_a, "B": args.apk_b, "F": args.apk_fail, "D": args.apk_dex}.items()}
    work = temp_dir("livebed-")
    keys = os.path.join(work, "keys")
    os.makedirs(keys)
    dt.run([dt.ANDYSU, "keygen", "--dir", keys])
    key = os.path.join(keys, "andysu-root.key")
    site = os.path.join(work, "site")
    os.makedirs(os.path.join(site, "stable"))
    port = dt.free_port()
    cfg = {"app": pkg, "channel": "stable", "roots": [open(os.path.join(keys, "andysu-public.pem")).read()],
           "manifest_url": f"http://127.0.0.1:{port}/stable/update.json", "mode": "auto", "apply": "live",
           "check_on_open": True, "watchdog_ms": 45000, "auto_healthy_ms": 60000, "max_boot_attempts": 2, "host_api": 1}
    cp = os.path.join(work, "config.json")
    json.dump(cfg, open(cp, "w"))
    tpl = os.path.join(dt.ANDROID, "host-template/build/andysu/andysu-host-template.zip")
    ks = ["--keystore", dt.debug_keystore(), "--ks-pass", "pass:android", "--key-alias", "androiddebugkey"]
    log("[live] splitting A (host with A built in) and the payloads of B, F and D")
    ra = json.loads(dt.run([dt.ANDYSU, "split", apk["A"], "--out", os.path.join(work, "A"), "--transparent", "--host-template", tpl,
                            "--config", cp, "--cleartext", *ks], timeout=3600))
    rel = {"A": ra["version"]}
    payload = {}
    for k in "BFD":
        r = json.loads(dt.run([dt.ANDYSU, "split", apk[k], "--out", os.path.join(work, k), "--transparent"], timeout=3600))
        rel[k] = r["version"]
        payload[k] = os.path.join(work, k, "payload")

    def publish(k):
        dt.run([dt.ANDYSU, "pack", payload[k], "--site", site, "--no-rename"], timeout=3600)
        dt.run([dt.ANDYSU, "publish", "--site", site, "--release", rel[k], "--key", key])
        log(f"[live] published {k} = {rel[k]}")

    # A is built into the host; B is published once A's engine is up (a check at open must not apply it
    # before the app registered its handler).
    d = dt.Device(args.api, 3, existing=args.device)
    srv = subprocess.Popen([dt.ANDYSU, "serve", site, "--addr", f"127.0.0.1:{port}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
    el = None
    res = {"api": args.api, "versions": rel, "steps": []}

    def step(name, detail):
        res["steps"].append({"step": name, "detail": detail})
        log(f"[live] OK {name}: {detail}")

    def launch():
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)

    def run_check():
        d.sh(f"cmd jobscheduler run -f {pkg} {JOB_ID}", check=False)

    def engine_up(mark, build_id, what, swapped=None, timeout=300):
        kw = {"build_id": build_id}
        if swapped is not None:
            kw["swapped"] = swapped
        st = wait_engine_start(el, mark, what, timeout, **kw)
        el.wait(r"========Initialized=========", f"{what}: init banner", mark, timeout=timeout)
        el.wait(r"GL_RENDERER: ", f"{what}: GL renderer", mark)
        wait_event(el, mark, "engine_frames", f"{what}: frames", build_id=build_id, timeout=timeout)
        return st

    def no_event(mark, names, seconds):
        time.sleep(seconds)
        bad = [e for _, e in events_since(el, mark) if e.get("event") in names]
        if bad:
            raise dt.Fail(f"unexpected events {names}: {bad}")

    def data_version():
        """The release the unpacked game data belongs to: GameData's stamp, written when an unpack finishes."""
        return d.sh(f"cat /sdcard/Android/data/{pkg}/files/.bundle-version 2>/dev/null", check=False).strip()

    try:
        d.start()
        no_immersive_prompt(d)
        d.fresh(pkg)
        if not d.existing:  # a shared device (--device) keeps its network: adb may run over it
            d.sh("svc wifi disable", check=False)
            d.sh("svc data disable", check=False)
        d.install(ra["host"]["apk"])
        seed_xonotic(d, pkg)
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        el = EngineLog(d)
        d.watch()

        # ---- 1. A boots from the built-in payload through the loader; B is staged at open (no handler yet)
        mark = el.mark()
        launch()
        _, ev = wait_event(el, mark, "booted", "A booted", version=rel["A"])
        if not (ev.get("transparent") and ev.get("factory")):
            raise dt.Fail(f"A should boot transparently from the built-in payload: {ev}")
        st_a = engine_up(mark, rel["A"], "A", swapped=0)
        pid = st_a["pid"]
        step("A built-in", f"{rel['A']} engine ran via the loader (SDL_main -> AndySU_SDL_main -> libmain), pid {pid}")
        screen_alive(d, f"live-A-builtin")
        time.sleep(5)

        # ---- 2. B is published; a check in the running process finds it native_live and swaps it in live
        publish("B")
        mark = el.mark()
        run_check()
        _, dec = wait_event(el, mark, "apply_decision", "decision for B", timeout=120)
        if not (dec["native_live"] and dec["pending"] == rel["B"] and dec["handler"] and dec["resumed"]):
            raise dt.Fail(f"B should be native_live with a handler registered: {dec}")
        wait_event(el, mark, "live_swap_requested", "host calls the handler", timeout=120, to=rel["B"])
        _, begin = wait_event(el, mark, "engine_swap_begin", "loader pre-flight")
        wait_event(el, mark, "engine_stopped", "old engine torn down")
        _, stB = wait_event(el, mark, "engine_start", "B started", swapped=1, build_id=rel["B"])
        wait_event(el, mark, "live_swapped", "host sees started()", timeout=120)
        engine_up(mark, rel["B"], "B (live)", swapped=1)
        screen_alive(d, f"live-B-swapped")
        _, com = wait_event(el, mark, "committed", "markHealthy commits B", timeout=180, version=rel["B"])
        if com["state"]["current"]["id"] != rel["B"] or com["state"].get("previous") or com["state"].get("trial"):
            raise dt.Fail(f"state after commit: {com['state']}")
        if stB["pid"] != pid or d.sh(f"pidof {pkg}", check=False).split() != [str(pid)]:
            raise dt.Fail(f"pid changed: {pid} -> {stB['pid']} / pidof {d.sh(f'pidof {pkg}', check=False)}")
        if data_version() != rel["B"]:
            raise dt.Fail(f"after the live swap to B the unpacked game data belongs to {data_version()!r}, expected {rel['B']}")
        step("live swap", f"{rel['A']} -> {rel['B']} in pid {pid}: live_swap_requested, engine_stopped, engine_start(swapped=1), "
             f"live_swapped, committed; state current={rel['B']} (A was the built-in payload, so no previous slot)")
        no_event(el.mark(), ("watchdog", "live_rollback", "relaunch"), 5)

        # ---- 3. broken B': in-process rollback, state back on B, B' bad, same process
        publish("F")
        mark = el.mark()
        run_check()
        wait_event(el, mark, "live_swap_requested", "host calls the handler for F", timeout=120, to=rel["F"])
        wait_event(el, mark, "engine_start_failed", "F's init fails", timeout=180)
        wait_event(el, mark, "engine_rolled_back", "loader reloads B", timeout=120)
        _, rbk = wait_event(el, mark, "live_rollback", "host: live_rollback", timeout=60)
        _, rb = wait_event(el, mark, "rolled_back", "host: state rolled back", timeout=60)
        stS = engine_up(mark, rel["B"], "B (after rollback)", swapped=2)
        screen_alive(d, f"live-B-rollback")
        if rb["state"]["current"]["id"] != rel["B"] or rel["F"] not in rb["state"]["bad"] or rb["state"].get("trial"):
            raise dt.Fail(f"state after rollback: {rb['state']}")
        if stS["pid"] != pid or d.sh(f"pidof {pkg}", check=False).split() != [str(pid)]:
            raise dt.Fail("the process changed during the in-process rollback")
        # The game data must belong to the release that runs again. F's start unpacked F's data first (its
        # stamp); the rollback has to put B's back before B's engine starts, not leave it to the next cold start.
        if data_version() != rel["B"]:
            raise dt.Fail(f"after the in-process rollback the unpacked game data belongs to {data_version()!r} (the failed "
                          f"release is {rel['F']}), expected {rel['B']}: the rollback did not prepare the previous release's data")
        _, gdF = wait_event(el, mark, "game_data_prepared", "F's data unpacked for the swap", timeout=30, swapped=1)
        _, gdB = wait_event(el, mark, "game_data_prepared", "B's data unpacked for the rollback", timeout=30, swapped=2)
        if gdF["data_version"] != rel["F"] or gdB["data_version"] != rel["B"] or gdB["release"] != rel["B"]:
            raise dt.Fail(f"game data events: F's start {gdF}, rollback {gdB}")
        if any(e.get("event") == "game_data_failed" for _, e in events_since(el, mark)):
            raise dt.Fail("a game_data_failed event during the rollback")
        step("in-process rollback", f"{rel['F']} failed its engine init ({rbk['reason'][:60]}); live_rollback; state current="
             f"{rel['B']}, bad={rb['state']['bad']}; game data {rel['F']} (its start) -> {rel['B']} (rollback, "
             f"{gdB['ms']} ms) before B's engine started again; engine {rel['B']} runs again in pid {pid}")

        # ---- 4. cold start boots B (F is bad)
        d.sh(f"am force-stop {pkg}")
        mark = el.mark()
        launch()
        _, ev = wait_event(el, mark, "booted", "cold start", version=rel["B"])
        if ev.get("trial"):
            raise dt.Fail(f"B is committed, not a trial: {ev}")
        st_cold = engine_up(mark, rel["B"], "B (cold start)", swapped=0)
        screen_alive(d, f"live-B-coldstart")
        step("cold start", f"boots {rel['B']} (committed), engine from slots/{rel['B']}: {st_cold['lib'].split('/andysu/')[-1]}")

        # ---- 5. a dex-changing release is not live: it waits for the next start
        publish("D")
        mark = el.mark()
        time.sleep(8)  # the check at open may already have run
        run_check()
        _, dec = wait_event(el, mark, "apply_decision", "decision for D", timeout=120)
        if dec["native_live"] or dec["pending"] != rel["D"]:
            raise dt.Fail(f"D changes the dex, so it is not native_live: {dec}")
        no_event(mark, ("live_swap_requested", "engine_swap_begin", "live_swapped", "relaunch"), 12)
        step("dex change", f"{rel['D']} staged with native_live=false: no live swap, falls back to the next start")
        d.sh(f"am force-stop {pkg}")
        mark = el.mark()
        launch()
        _, ev = wait_event(el, mark, "booted", "next start applies D", version=rel["D"])
        step("next start", f"{rel['D']} booted on the next start (trial={ev.get('trial')})")
        res["verdict"] = "live bed passes"
        log("live bed result: " + json.dumps(res, indent=1))
        os.makedirs(EVIDENCE, exist_ok=True)
        with open(os.path.join(EVIDENCE, "live-bed.json"), "w") as f:
            json.dump(res, f, indent=2)
    finally:
        stop_process(srv)
        try:
            if el:
                el.stop()
                el.save("live-bed-engine-log.txt")
        finally:
            if not args.keep:
                stop_device(d)


# ----------------------------------------------------------------------------- live failure paths (D29, milestone 2c)


def wait_status(el, since, state, timeout=120):
    """An `update_status` event whose status.state is `state`. -> (index, status)"""
    end = time.time() + timeout * dt.SLOW
    while time.time() < end:
        for i, e in events_since(el, since):
            if e.get("event") == "update_status" and e.get("status", {}).get("state") == state:
                return i, e["status"]
        time.sleep(0.5)
    raise dt.Fail(f"timed out waiting for update status {state}\n" + "\n".join(json.dumps(e)[:200] for _, e in events_since(el, 0)[-12:]))


def live_failures_bed(args):
    """The failure paths of an AndySU-driven live swap, one host install, apply=live:
       A -> B live; W (swaps in, never healthy): watchdog -> rollback -> relaunch -> B boots cold;
       C live from that cold B (GC keeps the booted slot); G (SIGSEGV after the swap): crash -> rollback
       -> relaunch -> C boots cold."""
    need = {"--apk-a": args.apk_a, "--apk-b": args.apk_b, "--apk-c": args.apk_c, "--apk-nohealthy": args.apk_nohealthy,
            "--apk-crash": args.apk_crash}
    missing = [k for k, v in need.items() if not v]
    if missing:
        raise SystemExit("--live-failures needs " + ", ".join(sorted(need)) + f" (missing {', '.join(missing)}); build them with "
                         "testbeds/patches/xonotic-touch/build.sh --variants \"1.2.9001 1.2.9002 1.2.9005 1.2.9006:nohealthy 1.2.9007:crash\"")
    pkg = BEDS["xonotic"]["pkg"]
    apk = {k: os.path.abspath(v) for k, v in {"A": args.apk_a, "B": args.apk_b, "C": args.apk_c, "W": args.apk_nohealthy,
                                               "G": args.apk_crash}.items()}
    work = temp_dir("livefail-")
    keys = os.path.join(work, "keys")
    os.makedirs(keys)
    dt.run([dt.ANDYSU, "keygen", "--dir", keys])
    key = os.path.join(keys, "andysu-root.key")
    site = os.path.join(work, "site")
    os.makedirs(os.path.join(site, "stable"))
    port = dt.free_port()
    wd_ms = 30000  # a healthy swapped-in engine reports within ~20 s on the emulator
    cfg = {"app": pkg, "channel": "stable", "roots": [open(os.path.join(keys, "andysu-public.pem")).read()],
           "manifest_url": f"http://127.0.0.1:{port}/stable/update.json", "mode": "auto", "apply": "live",
           "check_on_open": True, "watchdog_ms": wd_ms, "auto_healthy_ms": 600000, "max_boot_attempts": 2, "host_api": 1}
    cp = os.path.join(work, "config.json")
    json.dump(cfg, open(cp, "w"))
    tpl = os.path.join(dt.ANDROID, "host-template/build/andysu/andysu-host-template.zip")
    ks = ["--keystore", dt.debug_keystore(), "--ks-pass", "pass:android", "--key-alias", "androiddebugkey"]
    ra = json.loads(dt.run([dt.ANDYSU, "split", apk["A"], "--out", os.path.join(work, "A"), "--transparent", "--host-template", tpl,
                            "--config", cp, "--cleartext", *ks], timeout=3600))
    rel = {"A": ra["version"]}
    payload = {}
    for k in "BCWG":
        r = json.loads(dt.run([dt.ANDYSU, "split", apk[k], "--out", os.path.join(work, k), "--transparent"], timeout=3600))
        rel[k] = r["version"]
        payload[k] = os.path.join(work, k, "payload")

    def publish(k):
        dt.run([dt.ANDYSU, "pack", payload[k], "--site", site, "--no-rename"], timeout=3600)
        dt.run([dt.ANDYSU, "publish", "--site", site, "--release", rel[k], "--key", key])
        log(f"[fail] published {k} = {rel[k]}")

    d = dt.Device(args.api, 3, existing=args.device)
    srv = subprocess.Popen([dt.ANDYSU, "serve", site, "--addr", f"127.0.0.1:{port}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
    el = None
    res = {"api": args.api, "versions": rel, "steps": []}

    def step(name, detail):
        res["steps"].append({"step": name, "detail": detail})
        log(f"[fail] OK {name}: {detail}")

    def launch():
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)

    def run_check():
        d.sh(f"cmd jobscheduler run -f {pkg} {JOB_ID}", check=False)

    def engine_up(mark, build_id, what, swapped=None, timeout=300):
        kw = {"build_id": build_id}
        if swapped is not None:
            kw["swapped"] = swapped
        st = wait_engine_start(el, mark, what, timeout, **kw)
        el.wait(r"========Initialized=========", f"{what}: init banner", mark, timeout=timeout)
        el.wait(r"GL_RENDERER: ", f"{what}: GL renderer", mark)
        wait_event(el, mark, "engine_frames", f"{what}: frames", build_id=build_id, timeout=timeout)
        return st

    def live_to(k, mark_label):
        """publish k, check, and wait until it is swapped in (live_swapped); returns (mark, engine_start event)."""
        publish(k)
        mark = el.mark()
        run_check()
        wait_event(el, mark, "live_swap_requested", f"host calls the handler for {k}", timeout=120, to=rel[k])
        _, st = wait_event(el, mark, "engine_start", f"{k} started", swapped=1, build_id=rel[k], timeout=180)
        wait_event(el, mark, "live_swapped", f"host sees started() for {k}", timeout=120)
        return mark, st

    try:
        d.start()
        no_immersive_prompt(d)
        d.fresh(pkg)
        if not d.existing:  # a shared device (--device) keeps its network: adb may run over it
            d.sh("svc wifi disable", check=False)
            d.sh("svc data disable", check=False)
        d.install(ra["host"]["apk"])
        seed_xonotic(d, pkg)
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        el = EngineLog(d)
        d.watch()

        # ---- 0. A boots built-in; B swaps in live and commits (the precondition of every case below)
        mark = el.mark()
        launch()
        wait_event(el, mark, "booted", "A booted", version=rel["A"])
        st = engine_up(mark, rel["A"], "A", swapped=0)
        pid0 = st["pid"]
        screen_alive(d, f"fail-A-builtin")
        time.sleep(5)
        mark, _ = live_to("B", "B")
        wait_event(el, mark, "committed", "B commits", timeout=180, version=rel["B"])
        step("setup", f"{rel['A']} -> {rel['B']} live in pid {pid0}, committed")

        # ---- 1. watchdog: W swaps in but never reports healthy
        mark, stw = live_to("W", "W")
        if stw["pid"] != pid0:
            raise dt.Fail("W did not swap in the same process")
        t_swapped = time.time()
        _, wd = wait_event(el, mark, "watchdog", "watchdog on the unhealthy live trial", timeout=wd_ms // 1000 + 60)
        waited = time.time() - t_swapped
        _, rl = wait_event(el, mark, "relaunch", "Phoenix relaunch", timeout=60)
        _, ev = wait_event(el, mark, "booted", "relaunched process boots", timeout=120, version=rel["B"])
        if ev.get("trial") or ev.get("factory"):
            raise dt.Fail(f"B should boot committed from its slot after the watchdog rollback: {ev}")
        stb = engine_up(mark, rel["B"], "B (after watchdog)", swapped=0)
        screen_alive(d, f"fail-B-watchdog")
        if stb["pid"] == pid0:
            raise dt.Fail("the process was not relaunched")
        pid1 = stb["pid"]
        if "/slots/" + rel["B"] + "/" not in stb["lib"]:
            raise dt.Fail(f"B should run from its slot: {stb['lib']}")
        _, ls = wait_event(el, mark, "launch", "launch decision", timeout=60)
        _, utd = wait_status(el, mark, "up_to_date")
        step("watchdog", f"{rel['W']} swapped in (pid {pid0}) but never healthy: watchdog after {waited:.0f}s (watchdog_ms {wd_ms}), "
             f"reason '{rl['reason']}', relaunch -> pid {pid1}, {rel['B']} boots committed from slots/{rel['B']}, engine runs; "
             f"{rel['W']} is not offered again (up_to_date => marked bad)")

        # ---- 2. a live swap from a cold-started slot: the booted slot must stay on disk
        time.sleep(5)
        mark, _ = live_to("C", "C")
        _, com = wait_event(el, mark, "committed", "C commits", timeout=180, version=rel["C"])
        if com["state"]["current"]["id"] != rel["C"] or (com["state"].get("previous") or {}).get("id") != rel["B"]:
            raise dt.Fail(f"state after C: {com['state']}")
        # the GC after the commit must keep B (the slot this process booted from)
        end = time.time() + 60
        gc_ev = None
        while time.time() < end and gc_ev is None:
            for _, e in events_since(el, mark):
                if e.get("event") in ("gc", "gc_skipped"):
                    gc_ev = e
            time.sleep(1)
        if gc_ev is None:
            raise dt.Fail("no gc event after the commit")
        removed = (gc_ev.get("result") or {}).get("removed_slots", [])
        if rel["B"] in removed:
            raise dt.Fail(f"GC removed the booted slot {rel['B']}: {gc_ev}")
        step("second live swap", f"{rel['B']} (booted cold) -> {rel['C']} live in pid {pid1}; state current={rel['C']} previous="
             f"{rel['B']}; GC after commit: {json.dumps(gc_ev)[:160]} (booted slot kept)")

        # ---- 3. native crash after the swap
        time.sleep(5)
        mark, stg = live_to("G", "G")
        try:
            _, nc = wait_event(el, mark, "native_crash", "native crash reported to the host", timeout=90)
        except dt.Fail:
            tail = d.sh(f"ps -A | grep {pkg}", check=False)
            res["crash_gap"] = {"pid_alive": bool(d.sh(f"pidof {pkg}", check=False).strip()), "ps": tail[:200],
                                "events": [e.get("event") for _, e in events_since(el, mark)][-20:]}
            raise
        _, rl2 = wait_event(el, mark, "relaunch", "Phoenix relaunch after the crash", timeout=60)
        _, ev = wait_event(el, mark, "booted", "relaunched process boots", timeout=120, version=rel["C"])
        if ev.get("trial") or ev.get("factory"):
            raise dt.Fail(f"C should boot committed after the crash rollback: {ev}")
        stc = engine_up(mark, rel["C"], "C (after crash)", swapped=0)
        screen_alive(d, f"fail-C-crash")
        if stc["pid"] in (pid0, pid1):
            raise dt.Fail("the process was not relaunched after the crash")
        _, utd = wait_status(el, mark, "up_to_date")
        step("native crash", f"{rel['G']} swapped in (pid {pid1}), SIGSEGV 60 frames later: native_crash signal {nc['signal']}, relaunch "
             f"-> pid {stc['pid']}, {rel['C']} boots committed from its slot and the engine runs; {rel['G']} not offered again")
        res["verdict"] = "live failure paths pass"
        log("live failures result: " + json.dumps(res, indent=1))
        os.makedirs(EVIDENCE, exist_ok=True)
        with open(os.path.join(EVIDENCE, "live-failures.json"), "w") as f:
            json.dump(res, f, indent=2)
    finally:
        stop_process(srv)
        try:
            if el:
                el.stop()
                el.save("live-failures-engine-log.txt")
        finally:
            if not args.keep:
                stop_device(d)


# ----------------------------------------------------------------------------- low-storage GC guard (D29)


def low_storage_bed(args):
    """The GC guard under low storage, one host install, apply=live (AndySU.gc, Updater.allocatable):
       the app's storage figure is faked through <root>/test_allocatable_bytes (needs a debuggable system image:
       the file is written as root over `adb root`). Slot directories are checked on disk, not only by events.
       a. A -> B live, cold restart (B booted; no previous: A is the built-in payload, not a slot); 190 MiB 'free' (low); C swaps in live and commits:
          the GC runs low (low_storage=true) but keeps previous = B, the slot this process booted from
       b. D swaps in live in the same process (B is neither current nor previous): GC is skipped (gc_skipped),
          B stays on disk; then a pre-download GC (update E, 100 kB 'free') is skipped too, ending in no_space
       c. cold start (booted D): the same pre-download GC now drops previous (C), the normal low-storage rule"""
    need = {"--apk-a": args.apk_a, "--apk-b": args.apk_b, "--apk-c": args.apk_c, "--apk-d": args.apk_d, "--apk-e": args.apk_e}
    missing = [k for k, v in need.items() if not v]
    if missing:
        raise SystemExit("--low-storage needs " + ", ".join(sorted(need)) + f" (missing {', '.join(missing)}); build them with "
                         "testbeds/patches/xonotic-touch/build.sh --variants \"1.2.9001 1.2.9002 1.2.9005 1.2.9008 1.2.9007:crash\" "
                         "(E is never applied; any further native-only release does)")
    pkg = BEDS["xonotic"]["pkg"]
    apk = {k: os.path.abspath(v) for k, v in {"A": args.apk_a, "B": args.apk_b, "C": args.apk_c, "D": args.apk_d, "E": args.apk_e}.items()}
    work = temp_dir("lowstor-")
    keys = os.path.join(work, "keys")
    os.makedirs(keys)
    dt.run([dt.ANDYSU, "keygen", "--dir", keys])
    key = os.path.join(keys, "andysu-root.key")
    site = os.path.join(work, "site")
    os.makedirs(os.path.join(site, "stable"))
    port = dt.free_port()
    cfg = {"app": pkg, "channel": "stable", "roots": [open(os.path.join(keys, "andysu-public.pem")).read()],
           "manifest_url": f"http://127.0.0.1:{port}/stable/update.json", "mode": "auto", "apply": "live",
           "check_on_open": True, "watchdog_ms": 30000, "auto_healthy_ms": 600000, "max_boot_attempts": 2, "host_api": 1}
    cp = os.path.join(work, "config.json")
    json.dump(cfg, open(cp, "w"))
    tpl = os.path.join(dt.ANDROID, "host-template/build/andysu/andysu-host-template.zip")
    ks = ["--keystore", dt.debug_keystore(), "--ks-pass", "pass:android", "--key-alias", "androiddebugkey"]
    ra = json.loads(dt.run([dt.ANDYSU, "split", apk["A"], "--out", os.path.join(work, "A"), "--transparent", "--host-template", tpl,
                            "--config", cp, "--cleartext", *ks], timeout=3600))
    rel = {"A": ra["version"]}
    payload = {}
    for k in "BCDE":
        r = json.loads(dt.run([dt.ANDYSU, "split", apk[k], "--out", os.path.join(work, k), "--transparent"], timeout=3600))
        rel[k] = r["version"]
        payload[k] = os.path.join(work, k, "payload")
    rootdir = f"/data/data/{pkg}/no_backup/andysu"
    override = f"{rootdir}/test_allocatable_bytes"

    def publish(k):
        dt.run([dt.ANDYSU, "pack", payload[k], "--site", site, "--no-rename"], timeout=3600)
        dt.run([dt.ANDYSU, "publish", "--site", site, "--release", rel[k], "--key", key])
        log(f"[low] published {k} = {rel[k]}")

    d = dt.Device(args.api, 3, existing=args.device)
    srv = subprocess.Popen([dt.ANDYSU, "serve", site, "--addr", f"127.0.0.1:{port}"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)
    el = None
    res = {"api": args.api, "versions": rel, "steps": []}

    def step(name, detail):
        res["steps"].append({"step": name, "detail": detail})
        log(f"[low] OK {name}: {detail}")

    def launch():
        d.sh(f"monkey -p {pkg} -c android.intent.category.LAUNCHER 1", check=False)

    def run_check():
        d.sh(f"cmd jobscheduler run -f {pkg} {JOB_ID}", check=False)

    def engine_up(mark, build_id, what, swapped=None, timeout=300):
        kw = {"build_id": build_id}
        if swapped is not None:
            kw["swapped"] = swapped
        st = wait_engine_start(el, mark, what, timeout, **kw)
        el.wait(r"========Initialized=========", f"{what}: init banner", mark, timeout=timeout)
        el.wait(r"GL_RENDERER: ", f"{what}: GL renderer", mark)
        wait_event(el, mark, "engine_frames", f"{what}: frames", build_id=build_id, timeout=timeout)
        return st

    def live_to(k):
        publish(k)
        mark = el.mark()
        run_check()
        wait_event(el, mark, "live_swap_requested", f"host calls the handler for {k}", timeout=120, to=rel[k])
        wait_event(el, mark, "engine_start", f"{k} started", swapped=1, build_id=rel[k], timeout=180)
        wait_event(el, mark, "live_swapped", f"host sees started() for {k}", timeout=120)
        return mark

    def set_free(nbytes):
        """Fake the app's allocatable storage (root shell: the app's data dir is not reachable otherwise)."""
        if nbytes is None:
            d.sh(f"rm -f {override}")
            return
        d.sh(f"echo {nbytes} > {override} && chown $(stat -c %u:%g /data/data/{pkg}) {override} && chmod 600 {override} && "
             f"(restorecon {override} || true)")
        if d.sh(f"cat {override}").strip() != str(nbytes):
            raise dt.Fail("could not write the override file")

    def slots():
        return sorted(x for x in d.sh(f"ls {rootdir}/slots", check=False).split() if x)

    def state():
        return json.loads(d.sh(f"cat {rootdir}/state.json"))

    def pid():
        return d.sh(f"pidof {pkg}", check=False).strip()

    def wait_gc(mark, want, timeout=90):
        """first gc / gc_skipped event after `mark`; must be `want`."""
        end = time.time() + timeout * dt.SLOW
        while time.time() < end:
            for _, e in events_since(el, mark):
                if e.get("event") in ("gc", "gc_skipped"):
                    if e["event"] != want:
                        raise dt.Fail(f"expected {want}, got {json.dumps(e)}")
                    return e
            time.sleep(1)
        raise dt.Fail(f"no {want} event after the commit")

    def cold_start(what, version):
        d.sh(f"am force-stop {pkg}", check=False)
        time.sleep(2)
        if pid():
            raise dt.Fail("process survived force-stop")
        mark = el.mark()
        launch()
        _, ev = wait_event(el, mark, "booted", f"{what} booted", timeout=120, version=version)
        if ev.get("trial") or ev.get("factory"):
            raise dt.Fail(f"{version} should boot committed from its slot: {ev}")
        return mark, engine_up(mark, version, what, swapped=0)

    try:
        d.start()
        no_immersive_prompt(d)
        r = d.adb("root", check=False)
        if "cannot run as root" in r or "production builds" in r:
            raise dt.Fail(f"--low-storage needs `adb root` (a userdebug image, e.g. the API 34 aosp_atd one): {r.strip()}")
        d.adb("wait-for-device", timeout=60)
        time.sleep(3)
        d.fresh(pkg)
        if not d.existing:  # a shared device (--device) keeps its network: adb may run over it
            d.sh("svc wifi disable", check=False)
            d.sh("svc data disable", check=False)
        d.install(ra["host"]["apk"])
        seed_xonotic(d, pkg)
        d.adb("reverse", f"tcp:{port}", f"tcp:{port}")
        el = EngineLog(d)
        d.watch()

        # ---- 0. A boots built-in; B swaps in live, commits; then a cold start boots B from its slot
        mark = el.mark()
        launch()
        wait_event(el, mark, "booted", "A booted", version=rel["A"])
        st = engine_up(mark, rel["A"], "A", swapped=0)
        screen_alive(d, f"lowstor-A-builtin")
        time.sleep(5)
        mark = live_to("B")
        wait_event(el, mark, "committed", "B commits", timeout=180, version=rel["B"])
        _, stb = cold_start("B (cold)", rel["B"])
        screen_alive(d, f"lowstor-B-coldstart")
        if f"/slots/{rel['B']}/" not in stb["lib"]:
            raise dt.Fail(f"B should run from its slot: {stb['lib']}")
        pid1 = stb["pid"]
        st0 = state()
        if st0["current"]["id"] != rel["B"]:
            raise dt.Fail(f"state after the cold start: {st0}")
        step("setup", f"{rel['A']} -> {rel['B']} live, committed, cold restart: pid {pid1} runs {rel['B']} from slots/{rel['B']}; "
             f"slots on disk {slots()}, previous={(st0.get('previous') or {}).get('id')}")

        # ---- a. live swap to C under (faked) low storage: previous = B must survive the GC
        time.sleep(5)
        set_free(190 * 1024 * 1024)
        mark = live_to("C")
        wait_event(el, mark, "committed", "C commits", timeout=180, version=rel["C"])
        gc_ev = wait_gc(mark, "gc")
        if not gc_ev.get("low_storage"):
            raise dt.Fail(f"the GC should have run under (faked) low storage: {gc_ev}")
        stc = state()
        on_disk = slots()
        if stc["current"]["id"] != rel["C"] or (stc.get("previous") or {}).get("id") != rel["B"]:
            raise dt.Fail(f"state after C: {stc}")
        if rel["B"] not in on_disk or rel["C"] not in on_disk:
            raise dt.Fail(f"the booted slot {rel['B']} was removed by a low-storage GC: slots {on_disk}, gc {gc_ev}")
        if rel["B"] in gc_ev["result"].get("removed_slots", []):
            raise dt.Fail(f"GC reports the booted slot as removed: {gc_ev}")
        if pid() != str(pid1):
            raise dt.Fail("the process changed")
        step("low-storage GC keeps the booted slot", f"{rel['B']} (booted cold) -> {rel['C']} live in pid {pid1}; low_storage=true GC "
             f"removed {gc_ev['result'].get('removed_slots')}; state current={rel['C']} previous={rel['B']}; slots on disk {on_disk}")

        # ---- b. a second live swap: B is neither current nor previous any more
        time.sleep(5)
        mark = live_to("D")
        wait_event(el, mark, "committed", "D commits", timeout=180, version=rel["D"])
        gc_ev = wait_gc(mark, "gc_skipped")
        std = state()
        on_disk = slots()
        if std["current"]["id"] != rel["D"] or (std.get("previous") or {}).get("id") != rel["C"]:
            raise dt.Fail(f"state after D: {std}")
        if rel["B"] in (std["current"]["id"], (std.get("previous") or {}).get("id")):
            raise dt.Fail("the case needs B to be neither current nor previous")
        if rel["B"] not in on_disk:
            raise dt.Fail(f"the booted slot {rel['B']} is gone: {on_disk}")
        if rel["B"] not in gc_ev.get("reason", ""):
            raise dt.Fail(f"unexpected gc_skipped: {gc_ev}")
        step("GC skipped when the booted slot is not kept", f"{rel['C']} -> {rel['D']} live in pid {pid1}; state current={rel['D']} "
             f"previous={rel['C']}; {json.dumps(gc_ev)}; slots on disk {on_disk} (booted {rel['B']} still there)")

        # ---- b2. the other GC path: the updater frees space before a download (E needs space, 100 kB 'free')
        set_free(100_000)
        publish("E")
        mark = el.mark()
        run_check()
        gc_ev = wait_gc(mark, "gc_skipped", timeout=120)
        _, nospace = wait_status(el, mark, "error", timeout=120)
        if nospace.get("code") != "no_space":
            raise dt.Fail(f"expected no_space, got {nospace}")
        on_disk = slots()
        if rel["B"] not in on_disk:
            raise dt.Fail(f"the pre-download GC removed the booted slot: {on_disk}")
        if "overridden to 100000 bytes" not in d.sh("logcat -d -s AndySU:W", check=False):
            raise dt.Fail("the host did not log that the storage figure is overridden")
        step("pre-download GC skipped too", f"{rel['E']} needs space with 100 kB free: gc_skipped ({gc_ev.get('reason')}), status "
             f"no_space; slots on disk {on_disk}")

        # ---- c. cold start (booted D, nothing live-swapped): the same pre-download GC follows the normal rule
        mark, std2 = cold_start("D (cold)", rel["D"])
        screen_alive(d, f"lowstor-D-coldstart")
        _, nospace = wait_status(el, mark, "error", timeout=120)  # check_on_open sees E, needs space, GCs, fails
        if nospace.get("code") != "no_space":
            raise dt.Fail(f"expected no_space, got {nospace}")
        on_disk = slots()
        stz = state()
        if on_disk != [rel["D"]] or (stz.get("previous") or {}).get("id") != rel["C"]:
            # previous stays in state.json, but its slot goes; this is the core's rule for low storage
            raise dt.Fail(f"after a cold start a low-storage GC should leave only {rel['D']} on disk: {on_disk}, state {stz}")
        if any(e.get("event") == "gc_skipped" for _, e in events_since(el, mark)):
            raise dt.Fail("the GC must not be skipped in a freshly booted process")
        step("cold start drops previous", f"booted {rel['D']} (pid {std2['pid']}) from its slot; pre-download GC under low storage left "
             f"slots on disk {on_disk} (previous {rel['C']} dropped as usual)")

        set_free(None)
        if "No such file" not in d.sh(f"ls {override} 2>&1; true", check=False):
            raise dt.Fail("override file still there")
        res["verdict"] = "low-storage GC guard passes"
        log("low storage result: " + json.dumps(res, indent=1))
        os.makedirs(EVIDENCE, exist_ok=True)
        with open(os.path.join(EVIDENCE, "low-storage.json"), "w") as f:
            json.dump(res, f, indent=2)
    finally:
        stop_process(srv)
        try:
            if el:
                try:
                    set_free(None)
                except Exception:
                    pass
                el.stop()
                el.save("low-storage-engine-log.txt")
        finally:
            if not args.keep:
                stop_device(d)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", choices=list(BEDS))
    ap.add_argument("--api", default=None, help="default 34 (36 for the vid_restart probe: screenshots)")
    ap.add_argument("--device")
    ap.add_argument("--keep", action="store_true", help="keep the emulator running and the work dir")
    ap.add_argument("--apk-a", help="Xonotic APK to split as version A (default: testbeds/apks/...); locally built "
                    "APKs (testbeds/patches/xonotic-touch/build.sh) start the engine through the resident loader")
    ap.add_argument("--apk-b", help="Xonotic APK for the update, version B (needs --apk-a)")
    ap.add_argument("--live", action="store_true",
                    help="milestone 2b: AndySU drives the engine swap (needs --apk-a/--apk-b/--apk-fail/--apk-dex)")
    ap.add_argument("--apk-fail", help="with --live: engine whose init always fails (build.sh variant :fail)")
    ap.add_argument("--apk-dex", help="with --live: a release that changes the dex (build.sh variant :dex)")
    ap.add_argument("--live-failures", action="store_true",
                    help="milestone 2c: watchdog, native crash and second-swap cases of the live swap (needs --apk-a/b/c, "
                         "--apk-nohealthy, --apk-crash)")
    ap.add_argument("--apk-c", help="with --live-failures: a second native-only release after B")
    ap.add_argument("--apk-nohealthy", help="with --live-failures: engine that swaps in but never reports healthy (:nohealthy)")
    ap.add_argument("--apk-crash", help="with --live-failures: engine that SIGSEGVs after the swap (:crash)")
    ap.add_argument("--low-storage", action="store_true",
                    help="the GC guard under (faked) low storage after live swaps (needs --apk-a/b/c/d/e and a userdebug image)")
    ap.add_argument("--apk-d", help="with --low-storage: a third native-only release after C (a healthy one)")
    ap.add_argument("--apk-e", help="with --low-storage: one more release the updater is offered but never applies (any, e.g. :crash)")
    ap.add_argument("--live-swap-probe", action="store_true",
                    help="milestone 2a: in-process engine swaps A->B->A->B plus a forced-failure rollback (needs --apk-a/--apk-b)")
    ap.add_argument("--map", help="with --live-swap-probe: start this local map first and require it to be carried over every swap")
    ap.add_argument("--probe-vid-restart", action="store_true", help="R1 probe on the stock Xonotic APK (no AndySU)")
    args = ap.parse_args()
    global KEEP_TEMP_DIRS
    KEEP_TEMP_DIRS = args.keep
    atexit.register(cleanup_temp_dirs)
    dt.CONTAINER = not args.device
    if args.probe_vid_restart:
        args.api = args.api or "36"
        probe_vid_restart(args)
        return
    if args.live_failures:
        args.api = args.api or "36"
        dt.run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=dt.REPO)
        dt.gradle(":host-template:andysuHostTemplate")
        live_failures_bed(args)
        return
    if args.low_storage:
        args.api = args.api or "36"
        dt.run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=dt.REPO)
        dt.gradle(":host-template:andysuHostTemplate")
        low_storage_bed(args)
        return
    if args.live:
        args.api = args.api or "36"
        dt.run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=dt.REPO)
        dt.gradle(":host-template:andysuHostTemplate")
        live_bed(args)
        return
    if args.live_swap_probe:
        args.api = args.api or "36"
        live_swap_probe(args)
        return
    args.api = args.api or "34"
    if bool(args.apk_a) != bool(args.apk_b):
        ap.error("--apk-a and --apk-b go together")
    if args.apk_a:
        global LOADER
        LOADER = True
        BEDS["xonotic"]["a"] = os.path.abspath(args.apk_a)  # os.path.join(TB, absolute) is the absolute path
        BEDS["xonotic"]["b"] = os.path.abspath(args.apk_b)
    work = temp_dir("andysu-testbeds-")
    dt.run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=dt.REPO)
    dt.gradle(":host-template:andysuHostTemplate")
    d = dt.Device(args.api, 3, existing=args.device)
    results, failures = [], []
    try:
        d.start()
        no_immersive_prompt(d)
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
            stop_device(d)
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
