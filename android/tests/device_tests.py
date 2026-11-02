#!/usr/bin/env python3
"""Step 5 device tests: the AndySU Android host on real emulators, across API levels.

Builds the CLI, a series of test payloads (good, throwing, crashing in Java and in native code,
never healthy, crash looping, a large one, a content-only one), the factory slot and the test host
APK; serves a signed remote with `andysu serve` (reachable from the emulator at 10.0.2.2); then,
per API level, boots a headless emulator and checks:

  permissions   only normal permissions, nothing runtime/dangerous requested or granted
  factory       first launch runs the payload built into the APK
  live          live swap with state carried over, fresh copy of the renamed native dependency
  init-throw    start() throws -> live rollback in the same process
  native-crash  SIGSEGV on a payload thread during trial -> one relaunch -> previous slot
  java-crash    uncaught exception during trial -> one relaunch -> previous slot
  watchdog      markHealthy never called -> live rollback
  resume-kill   process killed mid-download -> download resumes from the partial file
  resume-net    connection dropped mid-download -> resumed with Range
  next-start    staged in the background, applied on the next launch, committed
  crash-loop    payload kills the process in start() -> boot counter -> rollback before loading
  hot           content-only update applied hot, payload notified
  reject        tampered, replayed (downgrade) and expired manifests rejected, app keeps running
  gc            only current + previous slots remain
  dcl           writable dex refused on API 34+, our read-only slots load everywhere
  16k           (16 KB page image) native libraries load on a 16 KB page kernel
  no-dialog     no permission, installer or crash dialog appeared during the whole run

Usage: python3 android/tests/device_tests.py [--api 24 29 31 34 36 36-16k] [--keep]
Writes a JSON report and a Markdown summary to android/tests/reports/.
"""
import argparse
import json
import os
import queue
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ANDROID = os.path.join(REPO, "android")
SDK = os.environ.get("ANDROID_HOME", os.path.expanduser("~/Android/Sdk"))
ADB = os.path.join(SDK, "platform-tools", "adb")
EMULATOR = os.path.join(SDK, "emulator", "emulator")
AVDMANAGER = os.path.join(SDK, "cmdline-tools", "latest", "bin", "avdmanager")
ANDYSU = os.path.join(REPO, "target", "debug", "andysu")
PKG = "com.andysu.testapp"
ACTIVITY = f"{PKG}/com.andysu.host.HostActivity"
NORMAL_PERMS = {"android.permission.INTERNET", "android.permission.ACCESS_NETWORK_STATE",
                "android.permission.RECEIVE_BOOT_COMPLETED"}
IMAGES = {
    "24": "system-images;android-24;default;x86_64",
    "29": "system-images;android-29;default;x86_64",
    "31": "system-images;android-31;aosp_atd;x86_64",
    "34": "system-images;android-34;aosp_atd;x86_64",
    "36": "system-images;android-36;google_apis;x86_64",
    "36-16k": "system-images;android-36;google_apis_ps16k;x86_64",
}


def log(msg):
    print(time.strftime("%H:%M:%S"), msg, flush=True)


def run(cmd, check=True, timeout=1800, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, **kw)
    if check and r.returncode != 0:
        raise RuntimeError(f"command failed ({r.returncode}): {' '.join(map(str, cmd))}\n{r.stdout[-3000:]}\n{r.stderr[-3000:]}")
    return r.stdout


def gradle(*args):
    return run([os.path.join(ANDROID, "gradlew"), "-q", "-p", ANDROID, "-Pandysu.abis=x86_64", *args], timeout=3600)


class Fail(Exception):
    pass


SLOW = 1.0  # --slow: multiplies every timeout (software-emulated devices)
CONTAINER = False  # --container: run emulators inside the andysu-emulator podman image


# ----------------------------------------------------------------------------- build


class Build:
    def __init__(self, work):
        self.work = work
        self.payloads = {}

    def cli(self):
        run(["cargo", "build", "-q", "-p", "andysu-cli"], cwd=REPO)

    def keys(self):
        d = os.path.join(self.work, "keys")
        os.makedirs(d, exist_ok=True)
        run([ANDYSU, "keygen", "--dir", d])
        self.key = os.path.join(d, "andysu-root.key")
        self.pub = os.path.join(d, "andysu-public.pem")

    def payload(self, version, behavior="ok", blob_kb=0):
        log(f"building payload {version} ({behavior})")
        gradle(":testapp:payload:andysuPayloadRelease", f"-Ptp.version={version}", f"-Ptp.behavior={behavior}",
               f"-Ptp.blobKb={blob_kb}")
        out = os.path.join(self.work, "payloads", version)
        shutil.rmtree(out, ignore_errors=True)
        shutil.copytree(os.path.join(ANDROID, "testapp/payload/build/andysu/payload/release"), out)
        self.payloads[version] = out
        return out

    def content_only(self, base_version, version, motd):
        """Same code as base, new motd.txt and version: a hot (tier A) update."""
        out = os.path.join(self.work, "payloads", version)
        shutil.rmtree(out, ignore_errors=True)
        shutil.copytree(self.payloads[base_version], out)
        with open(os.path.join(out, "assets", "motd.txt"), "w") as f:
            f.write(motd + "\n")
        p = os.path.join(out, "andysu-payload.json")
        d = json.load(open(p))
        d["version"] = version
        json.dump(d, open(p, "w"), indent=2)
        self.payloads[version] = out
        return out

    def host(self, manifest_url):
        log("building factory slot and host APK")
        site = os.path.join(self.work, "factory-site")
        slot = os.path.join(self.work, "factory-slot")
        shutil.rmtree(site, ignore_errors=True)
        run([ANDYSU, "pack", self.payloads["0.9"], "--site", site, "--emit-slot", slot, "--require-16k"])
        cfg = {
            "app": PKG, "channel": "stable", "roots": [open(self.pub).read()], "manifest_url": manifest_url,
            "mode": "auto", "apply": "live", "network": "any", "check_on_open": True,
            "check_interval_minutes": 360, "watchdog_ms": 6000, "max_boot_attempts": 2, "host_api": 1,
        }
        cp = os.path.join(self.work, "config.json")
        json.dump(cfg, open(cp, "w"), indent=2)
        gradle(":testapp:app:assembleDebug", f"-Pandysu.config={cp}", f"-Pandysu.factory={slot}")
        gradle(":host-template:andysuHostTemplate")
        self.template = os.path.join(ANDROID, "host-template/build/andysu/andysu-host-template.zip")
        self.apk = os.path.join(ANDROID, "testapp/app/build/outputs/apk/debug/app-debug.apk")
        # andysu split on a finished APK (the test payload's own release APK): host + payload.
        self.split_dir = os.path.join(self.work, "split")
        shutil.rmtree(self.split_dir, ignore_errors=True)
        rep = json.loads(run([ANDYSU, "split", os.path.join(ANDROID, "testapp/payload/build/outputs/apk/release/payload-release-unsigned.apk"),
                              "--out", self.split_dir, "--entry-class", "com.andysu.testpayload.TestPayload",
                              "--host-template", self.template, "--config", cp,
                              "--keystore", os.path.expanduser("~/.android/debug.keystore"), "--ks-pass", "pass:android",
                              "--key-alias", "androiddebugkey", "--cleartext"]))
        self.split_apk = rep["host"]["apk"]
        self.split_version = rep["version"]
        self.host_json = os.path.join(self.work, "host.json")
        json.dump({"app": PKG, "host_api": 1, "permissions": sorted(NORMAL_PERMS), "activities": 4, "services": 2,
                   "receivers": 2, "providers": 1}, open(self.host_json, "w"))


# ----------------------------------------------------------------------------- remote


class Remote:
    def __init__(self, build, port):
        self.b = build
        self.port = port
        self.site = None
        self.srv = None

    def reset(self):
        self.stop()
        self.site = tempfile.mkdtemp(prefix="site-", dir=self.b.work)
        os.makedirs(os.path.join(self.site, "stable"), exist_ok=True)
        self.serve()

    def serve(self, *extra):
        self.stop()
        self.srv = subprocess.Popen([ANDYSU, "serve", self.site, "--addr", f"127.0.0.1:{self.port}", *extra],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(0.4)

    def stop(self):
        if self.srv:
            self.srv.kill()
            self.srv.wait()
            self.srv = None

    def publish(self, version, hot=False, expires_days=30):
        args = [ANDYSU, "pack", self.b.payloads[version], "--site", self.site, "--host", self.b.host_json,
                "--require-16k"]
        if hot:
            args += ["--hot", "assets/"]
        run(args)
        run([ANDYSU, "publish", "--site", self.site, "--release", version, "--key", self.b.key,
             "--expires-days", str(expires_days)])

    def sign(self, *extra):
        run([ANDYSU, "sign", "--site", self.site, "--key", self.b.key, *extra])

    def manifest_path(self):
        return os.path.join(self.site, "stable", "update.json")


# ----------------------------------------------------------------------------- device


class Device:
    def __init__(self, api, idx, existing=None):
        self.existing = existing
        self.api = api
        self.port = 5580 + 2 * idx
        self.serial = existing or f"emulator-{self.port}"
        self.name = f"andysu-test-{api}"
        self.emu = None
        self.q = queue.Queue()
        self.events = []
        self.logcat = None
        self.focus_log = []
        self._stop_focus = threading.Event()

    def adb(self, *args, check=True, timeout=120):
        return run([ADB, "-s", self.serial, *args], check=check, timeout=timeout * SLOW)

    def sh(self, cmd, check=True, timeout=120):
        return self.adb("shell", cmd, check=check, timeout=timeout)

    def start(self):
        if self.existing:
            self.adb("wait-for-device", timeout=60)
            self.restore_focus = self.sh("dumpsys window | grep mCurrentFocus", check=False).strip()
            self._booted()
            return
        img = IMAGES[self.api]
        avds = run([EMULATOR, "-list-avds"])
        if self.name not in avds.split():
            log(f"[{self.api}] creating AVD")
            subprocess.run([AVDMANAGER, "create", "avd", "-n", self.name, "-k", img, "-d", "pixel_6", "--force"],
                           input="no\n", text=True, capture_output=True, check=True)
        log(f"[{self.api}] booting emulator{' (container)' if CONTAINER else ''}")
        common = ["-wipe-data", "-memory", "3072", "-cores", "4"]
        if CONTAINER:
            cmd = [os.path.join(ANDROID, "tests", "emulator", "run.sh"), self.name, str(self.port), *common]
        else:
            cmd = [EMULATOR, "-avd", self.name, "-port", str(self.port), "-no-window", "-no-audio", "-no-boot-anim",
                   "-no-snapshot", "-gpu", "swiftshader_indirect", *common]
        self.emu = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.adb("wait-for-device", timeout=300)
        end = time.time() + 600
        while time.time() < end:
            if self.sh("getprop sys.boot_completed", check=False).strip() == "1":
                break
            time.sleep(2)
        else:
            raise Fail("emulator did not boot")
        self._booted()

    def _booted(self):
        self.sh("settings put global window_animation_scale 0", check=False)
        self.sh("settings put global transition_animation_scale 0", check=False)
        self.sh("settings put global animator_duration_scale 0", check=False)
        self.sh("input keyevent KEYCODE_WAKEUP", check=False)
        self.sh("wm dismiss-keyguard", check=False)
        self.sdk = int(self.sh("getprop ro.build.version.sdk").strip())
        self.page_size = int(self.sh("getconf PAGE_SIZE", check=False).strip() or "4096")
        log(f"[{self.api}] booted: SDK {self.sdk}, page size {self.page_size}")
        threading.Thread(target=self._focus_watch, daemon=True).start()

    def stop(self, keep=False):
        self._stop_focus.set()
        if self.logcat:
            self.logcat.kill()
        if self.existing:
            # Leave a shared device as we found it.
            self.adb("uninstall", PKG, check=False)
            self.adb("reverse", "--remove-all", check=False)
            comp = self.restore_focus.split()[-1].rstrip("}") if self.restore_focus else ""
            if "/" in comp and PKG not in comp:
                self.sh(f"am start -n {comp}", check=False)
            return
        try:
            self.adb("emu", "kill", check=False, timeout=30)
        except Exception:
            pass
        if self.emu:
            try:
                self.emu.wait(timeout=60)
            except subprocess.TimeoutExpired:
                self.emu.kill()
            if CONTAINER:
                subprocess.run(["podman", "stop", "-t", "5", f"andysu-emu-{self.port}"], capture_output=True)

    def _focus_watch(self):
        while not self._stop_focus.is_set():
            try:
                out = run([ADB, "-s", self.serial, "shell", "dumpsys window | grep -E 'mCurrentFocus|mFocusedApp'"],
                          check=False, timeout=20)
                for line in out.splitlines():
                    line = line.strip()
                    if line and (not self.focus_log or self.focus_log[-1] != line):
                        self.focus_log.append(line)
            except Exception:
                pass
            time.sleep(0.3)

    def diagnostics(self):
        out = []
        try:
            out.append(self.sh(f"ps -A -o PID,STAT,WCHAN,NAME | grep -E 'andysu|PID'", check=False, timeout=20))
            for pid in self.sh(f"pidof {PKG}", check=False, timeout=20).split():
                self.sh(f"run-as {PKG} kill -3 {pid}", check=False, timeout=20)
            time.sleep(2)
            out.append(self.adb("logcat", "-d", "-t", "400", check=False, timeout=30))
        except Exception as e:
            out.append(f"(diagnostics failed: {e})")
        keep = [l for l in "\n".join(out).splitlines() if any(k in l for k in
                ["andysu", "AndySU", "AndroidRuntime", "ANR", "FATAL", "\"main\"", "  at ", "PID", "Phoenix", "DEBUG"])]
        return "\n".join(keep[-150:])

    # -- app control

    def install(self, apk):
        self.adb("install", "-r", "-t", apk, timeout=300)

    def clear(self):
        self.sh(f"pm clear {PKG}")

    def launch(self):
        self.sh(f"am start -n {ACTIVITY}")

    def home(self):
        self.sh("input keyevent KEYCODE_HOME")

    def force_stop(self):
        self.sh(f"am force-stop {PKG}")

    def broadcast(self, action, **extras):
        # -f 0x20: FLAG_INCLUDE_STOPPED_PACKAGES (the app may have been force-stopped)
        es = "".join(f" --es {k} {v}" for k, v in extras.items())
        self.sh(f"am broadcast -f 0x20 -n {PKG}/.TestCommands -a {action}{es}", timeout=45)

    def state(self):
        return json.loads(self.sh(f"run-as {PKG} cat no_backup/andysu/state.json"))

    def ls(self, rel):
        return self.sh(f"run-as {PKG} ls -l no_backup/andysu/{rel}", check=False)

    # -- events

    def watch(self):
        if self.logcat:
            self.logcat.kill()
        self.adb("logcat", "-c")
        self.q = queue.Queue()
        self.events = []
        self.logcat = subprocess.Popen([ADB, "-s", self.serial, "logcat", "-v", "raw", "-s", "AndySU-Event:I"],
                                       stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)

        def reader(p):
            for line in p.stdout:
                line = line.strip()
                if line.startswith("{"):
                    try:
                        ev = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    self.events.append(ev)
                    self.q.put(ev)

        threading.Thread(target=reader, args=(self.logcat,), daemon=True).start()

    def wait(self, pred, what, timeout=60):
        end = time.time() + timeout * SLOW
        while time.time() < end:
            try:
                ev = self.q.get(timeout=0.5)
            except queue.Empty:
                continue
            if pred(ev):
                return ev
        tail = "\n".join(json.dumps(e)[:300] for e in self.events[-20:])
        raise Fail(f"timed out waiting for {what}\n{tail}")

    def payload(self, version, timeout=90):
        ev = self.wait(lambda e: e.get("event") == "payload_started" and e.get("version") == version,
                       f"payload {version} started", timeout)
        self.last_payload = ev
        return ev

    def event(self, name, timeout=60, **match):
        return self.wait(lambda e: e.get("event") == name and all(e.get(k) == v for k, v in match.items()),
                         f"event {name} {match}", timeout)

    def status(self, state, timeout=120):
        return self.wait(lambda e: e.get("event") == "update_status" and e["status"].get("state") == state,
                         f"update status {state}", timeout)


# ----------------------------------------------------------------------------- scenarios


def scenarios(d, b, r, results):
    def ok(name, detail=""):
        results.append({"api": d.api, "sdk": d.sdk, "scenario": name, "ok": True, "detail": detail})
        log(f"[{d.api}] ok  {name}  {detail}")

    def count(name):
        return sum(1 for e in d.events if e.get("event") == name)

    d.install(b.apk)
    d.adb("reverse", f"tcp:{r.port}", f"tcp:{r.port}")
    d.clear()
    r.reset()
    d.watch()

    # permissions
    dump = d.sh(f"dumpsys package {PKG}")
    requested = set()
    section = None
    for line in dump.splitlines():
        s = line.strip()
        if s.startswith("requested permissions:"):
            section = "req"
            continue
        if section == "req":
            if s.startswith("android.permission") or s.startswith(PKG):
                requested.add(s.split(":")[0])
                continue
            section = None
    dangerous = set(l.strip()[len("permission:"):] for l in d.sh("pm list permissions -d -g").splitlines()
                    if l.strip().startswith("permission:"))
    bad = requested & dangerous
    extra = {p for p in requested if p not in NORMAL_PERMS and not p.startswith(PKG)}
    if bad or extra:
        raise Fail(f"unexpected permissions requested: dangerous={bad} other={extra}")
    if "runtime permissions:" in dump and any("granted=true" in l for l in dump.split("runtime permissions:")[1].split("\n\n")[0].splitlines()):
        raise Fail("a runtime permission is granted")
    ok("permissions", f"requested {sorted(requested)}; none dangerous; no runtime grants")

    # factory
    d.launch()
    ev = d.payload("0.9")
    if not ev.get("factory"):
        raise Fail(f"0.9 should run as factory: {ev}")
    dcl_factory = ev["dcl_writable"]
    ok("factory", f"built-in payload 0.9 runs (loader {ev['loader'].split('.')[-1]}, Kotlin {ev['kotlin']})")

    # live update to 1.0
    r.publish("1.0")
    time.sleep(1.5)
    d.broadcast("check")
    d.event("live_swapped", to="1.0")
    ev = d.payload("1.0")
    if not (ev["counter"] > 0 and ev["dep_version"] == 1 and ev["dep_instances"] == 1):
        raise Fail(f"bad live start: {ev}")
    d.event("committed", version="1.0")
    ok("live", f"0.9 -> 1.0 live, counter carried ({ev['counter']}), fresh libtestdep copy, committed")

    # init exception -> live rollback
    r.publish("2.0")
    d.broadcast("check")
    rb = d.event("live_rollback")
    if "start() threw" not in rb.get("reason", ""):
        raise Fail(f"unexpected rollback reason {rb}")
    d.payload("1.0")
    if "2.0" not in d.state()["bad"]:
        raise Fail("2.0 not marked bad")
    ok("init-throw", "2.0 start() threw -> live rollback to 1.0 in-process, 2.0 marked bad")

    # native crash during trial -> one relaunch
    inits = count("init")
    r.publish("3.0")
    d.broadcast("check")
    d.payload("3.0")
    d.event("native_crash")
    d.event("relaunch")
    launch = d.event("launch", timeout=60)
    evs = launch.get("events", [])
    if not any(e.get("event") == "rolled_back" and "native crash" in e.get("reason", "") for e in evs):
        raise Fail(f"launch after native crash did not roll back: {launch}")
    d.payload("1.0")
    new_procs = count("init") - inits
    if new_procs != 1:
        raise Fail(f"expected exactly one relaunch, saw {new_procs} process starts")
    ok("native-crash", "SIGSEGV on a payload thread in trial -> marker -> exactly one relaunch -> 1.0")

    # Java crash during trial -> one relaunch
    inits = count("init")
    r.publish("4.0")
    d.broadcast("check")
    d.payload("4.0")
    d.event("uncaught")
    launch = d.event("launch", timeout=60)
    d.payload("1.0")
    if count("init") - inits != 1:
        raise Fail("expected exactly one relaunch after the Java crash")
    if "4.0" not in d.state()["bad"]:
        raise Fail("4.0 not marked bad")
    ok("java-crash", "uncaught exception on the main thread in trial -> one relaunch -> 1.0")

    # watchdog
    r.publish("5.0")
    d.broadcast("check")
    d.payload("5.0")
    d.event("watchdog", timeout=30)
    d.event("live_rollback")
    d.payload("1.0")
    ok("watchdog", "5.0 never called markHealthy -> watchdog -> live rollback")

    # resume after process kill, while the app is in the background (next-start apply)
    r.serve("--throttle-kbps", "2048")
    r.publish("7.0")
    d.home()
    time.sleep(1)
    d.broadcast("check")
    d.status("downloading")
    time.sleep(3 * SLOW)
    d.force_stop()
    parts = d.ls("blobs/sha256/")
    if ".part" not in parts:
        raise Fail(f"no partial download after the kill:\n{parts}")
    r.serve()
    d.broadcast("check")
    res = d.event("resume", timeout=60)
    d.status("staged", timeout=180)
    ok("resume-kill", f"process killed mid-download; resumed at offset {res['offset']}")
    st = d.state()
    if (st.get("pending") or {}).get("id") != "7.0" or (st.get("current") or {}).get("id") != "1.0":
        raise Fail(f"7.0 should be pending while 1.0 runs: {st}")
    d.force_stop()
    d.launch()
    launch = d.event("launch")
    if not launch.get("in_trial"):
        raise Fail(f"7.0 should boot in trial: {launch}")
    ev = d.payload("7.0")
    d.event("committed", version="7.0")
    ok("next-start", "7.0 staged in the background, applied on the next launch in trial, committed")

    # network loss mid-download (connection dropped), resumed with Range
    r.serve("--drop-after", "300000", "--drop-prefix", "blobs/")
    r.publish("7.5")
    d.home()
    d.broadcast("check")
    d.event("download_interrupted", timeout=120)
    d.event("resume", timeout=60)
    d.status("staged", timeout=180)
    r.serve()
    d.force_stop()
    d.launch()
    d.payload("7.5")
    d.event("committed", version="7.5")
    ok("resume-net", "connection dropped after 300 KB; resumed with Range; 7.5 committed on next start")

    # crash loop: the payload kills the process in start(), no handler sees it
    r.publish("6.0")
    d.home()
    d.broadcast("check")
    d.status("staged")
    d.force_stop()
    launches = 0
    for _ in range(3):
        d.launch()
        launches += 1
        launch = d.event("launch")
        if any(e.get("event") == "rolled_back" for e in launch.get("events", [])):
            break
        d.event("payload_exiting")
        time.sleep(2)
    d.payload("7.5")
    st = d.state()
    if "6.0" not in st["bad"] or launches != 3:
        raise Fail(f"crash loop not stopped as expected after {launches} launches: {st}")
    ok("crash-loop", "6.0 killed the process in start() twice; 3rd launch rolled back before loading -> 7.5")

    # hot content update
    b.content_only("7.5", "7.6", "hot motd 7.6")
    r.publish("7.6", hot=True)
    d.broadcast("check")
    d.event("hot_applied")
    ev = d.event("content_updated")
    if ev.get("motd") != "hot motd 7.6":
        raise Fail(f"hot content not visible: {ev}")
    d.event("committed")
    ok("hot", "content-only 7.6 applied hot (no restart), payload re-read assets")

    # rejections
    good = open(r.manifest_path(), "rb").read()
    good_sig = open(r.manifest_path() + ".sig", "rb").read()
    bad = bytearray(good)
    bad[len(bad) // 2] ^= 1
    open(r.manifest_path(), "wb").write(bytes(bad))
    d.broadcast("check")
    e = d.status("error")
    if e["status"].get("code") != "bad_signature":
        raise Fail(f"tamper: {e}")
    open(r.manifest_path(), "wb").write(good)
    r.sign()
    d.broadcast("check")
    d.status("up_to_date")
    open(r.manifest_path(), "wb").write(good)
    open(r.manifest_path() + ".sig", "wb").write(good_sig)
    d.broadcast("check")
    e = d.status("error")
    if e["status"].get("code") != "sequence_rollback":
        raise Fail(f"replay: {e}")
    r.sign("--expires-days", "0")
    d.broadcast("check")
    e = d.status("error")
    if e["status"].get("code") != "expired":
        raise Fail(f"expired: {e}")
    r.sign()
    if (d.state().get("current") or {}).get("id") != "7.6":
        raise Fail("current changed after rejected manifests")
    ok("reject", "tampered -> bad_signature, replayed -> sequence_rollback, expired -> expired; 7.6 kept running")

    # gc
    d.broadcast("gc")
    d.event("gc")
    slots = sorted(l.split()[-1] for l in d.ls("slots/").splitlines() if l.startswith("d"))
    st = d.state()
    want = sorted(x["id"] for x in [st.get("current"), st.get("previous")] if x)
    if slots != want:
        raise Fail(f"slots on disk {slots}, expected {want}")
    ok("gc", f"slots on disk after GC: {slots}")

    # read-only dynamic code loading
    if d.sdk >= 34 and not dcl_factory.startswith("refused"):
        raise Fail(f"API {d.sdk}: writable dex was loaded: {dcl_factory}")
    ok("dcl", f"writable dex probe: {dcl_factory}; read-only slots load")

    if d.page_size == 16384:
        if d.last_payload.get("page_size") != 16384:
            raise Fail(f"payload saw page size {d.last_payload.get('page_size')}")
        ok("16k", "16 KB page kernel: core, host bridge and renamed payload libraries all loaded and ran")

    # apply policy = restart (set at runtime): one relaunch applies the update
    d.broadcast("policy", policy="restart")
    d.event("apply_policy", policy="restart")
    inits = count("init")
    r.publish("8.0")
    d.broadcast("check")
    d.wait(lambda e: e.get("event") == "relaunch" and e.get("reason") == "applying update", "restart to apply")
    launch = d.event("launch")
    if not launch.get("in_trial"):
        raise Fail(f"8.0 should boot in trial after the restart: {launch}")
    d.payload("8.0")
    d.event("committed", version="8.0")
    if count("init") - inits != 1:
        raise Fail("expected exactly one restart")
    ok("apply-restart", "policy restart: 8.0 staged, the app restarted itself once, booted in trial, committed")

    # apply policy = next_start while the app is in the foreground: staged, not applied until the next start
    d.broadcast("policy", policy="next_start")
    d.event("apply_policy", policy="next_start")
    r.publish("8.5")
    d.broadcast("check")
    d.status("staged")
    time.sleep(3 * SLOW)
    if any(e.get("event") in ("live_swapped", "relaunch") for e in d.events[-30:]):
        raise Fail("next_start policy applied the update early")
    d.force_stop()
    d.launch()
    d.payload("8.5")
    d.event("committed", version="8.5")
    d.broadcast("policy")  # back to the configured policy (live)
    d.event("apply_policy", policy="live")
    ok("apply-next-start", "policy next_start: 8.5 staged while the app was in front, applied on the next start; reset to live")

    # andysu split: the host built from a finished APK boots its built-in payload and updates
    d.adb("uninstall", PKG, check=False)
    d.install(b.split_apk)
    d.launch()
    ev = d.payload(b.split_version)
    if not ev.get("factory"):
        raise Fail(f"split host should start on its built-in payload: {ev}")
    d.event("live_swapped", to="8.5", timeout=120)
    d.payload("8.5")
    d.event("committed")
    ok("split", f"host APK made by `andysu split` from the payload's own APK: built-in {b.split_version} ran, live update to 8.5 committed")

    # no dialogs
    bad_focus = [f for f in d.focus_log if any(x in f.lower() for x in
                 ["packageinstaller", "permissioncontroller", "grantpermissions", "application not responding",
                  "has stopped", "keeps stopping", "aerr", "anr"])]
    if bad_focus:
        raise Fail(f"a dialog appeared: {bad_focus}")
    ok("no-dialog", f"{len(d.focus_log)} focus changes observed, none to an installer, permission or crash dialog")


# ----------------------------------------------------------------------------- main


def free_port():
    s = socket.socket()
    s.bind(("0.0.0.0", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--api", nargs="+", default=["24", "29", "31", "34", "36", "36-16k"])
    ap.add_argument("--keep", action="store_true", help="keep the work directory")
    ap.add_argument("--device", help="run on this already-running device (adb serial) instead of emulators")
    ap.add_argument("--container", action="store_true", help="run emulators in a podman container (tests/emulator)")
    ap.add_argument("--slow", type=float, default=1.0, help="timeout multiplier for slow (TCG) devices")
    args = ap.parse_args()
    global SLOW, CONTAINER
    SLOW = args.slow
    CONTAINER = args.container

    work = tempfile.mkdtemp(prefix="andysu-device-")
    b = Build(work)
    b.cli()
    b.keys()
    port = free_port()
    r = Remote(b, port)
    for v, beh in [("0.9", "ok"), ("1.0", "ok"), ("2.0", "throw_start"), ("3.0", "crash_native"),
                   ("4.0", "crash_java"), ("5.0", "never_healthy"), ("6.0", "exit_on_start")]:
        b.payload(v, beh)
    b.payload("7.0", "ok", blob_kb=20000)
    b.payload("7.5", "ok", blob_kb=6000)
    # The remote is reached through `adb reverse`, the same on emulators, VMs and phones.
    b.host(f"http://127.0.0.1:{port}/stable/update.json")
    # Built after the host: `split` above used the 7.5 APK, so these are real updates for it too.
    b.payload("8.0", "ok")
    b.payload("8.5", "ok")

    results, failures = [], []
    targets = [args.device] if args.device else args.api
    for i, api in enumerate(targets):
        d = Device(api, i, existing=args.device)
        try:
            d.start()
            scenarios(d, b, r, results)
        except Exception as e:
            diag = d.diagnostics()
            failures.append({"api": api, "error": str(e), "diagnostics": diag})
            log(f"[{api}] FAILED: {e}\n--- diagnostics ---\n{diag}")
        finally:
            r.stop()
            d.stop()

    os.makedirs(os.path.join(ANDROID, "tests", "reports"), exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    rep = {"when": stamp, "results": results, "failures": failures}
    with open(os.path.join(ANDROID, "tests", "reports", f"device-{stamp}.json"), "w") as f:
        json.dump(rep, f, indent=2)
    with open(os.path.join(ANDROID, "tests", "reports", f"device-{stamp}.md"), "w") as f:
        f.write(f"# Device test run {stamp}\n\n| API | SDK | scenario | detail |\n|---|---|---|---|\n")
        for x in results:
            f.write(f"| {x['api']} | {x['sdk']} | {x['scenario']} | {x['detail']} |\n")
        for x in failures:
            f.write(f"\n**FAILED on {x['api']}:**\n\n```\n{x['error'][:4000]}\n```\n")
    if not args.keep:
        shutil.rmtree(work, ignore_errors=True)
    log(f"{len(results)} checks passed, {len(failures)} API levels failed")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
