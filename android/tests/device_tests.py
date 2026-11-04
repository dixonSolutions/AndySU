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
  crash-handler payload replaces the SIGSEGV handler: reported only by default (a handling one commits, a hanging one
                is rolled back by the watchdog); crash_signals = host re-installs, chains it, marker + relaunch -> rollback
  watchdog      markHealthy never called -> live rollback
  background-trial  app sent HOME during a trial, away longer than watchdog_ms: the watchdog pauses (watchdog_paused),
                resumes on return (watchdog_resumed), the release commits, no rollback (D31)
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

When a scenario fails, android/tests/reports/ also gets, per API level (stamp = run time):
  device-<stamp>-api<API>-logcat.txt    full logcat (threadtime) since the last `logcat -c`
  device-<stamp>-api<API>-events.jsonl  every AndySU-Event the harness saw
  device-<stamp>-api<API>-threads.txt   Java thread stacks of the app (SIGQUIT) and native stacks (root only)
  device-<stamp>-api<API>-appdata.txt   no_backup/andysu listing, state.json, state.lock present or not
  device-<stamp>-api<API>-server.log    `andysu serve` request log / stderr
The job log names these files. CI uploads the whole reports/ directory.

ANDYSU_TEST_IGNORE_EVENT=<event>[:<state>[:<release>]] (e.g. update_status:staged:7.0) drops the
first such host log line in the harness, to exercise the state.json fallback for lost host lines (liblog drops
writes while logd is busy).
ANDYSU_TEST_FORCE_FAIL=<scenario> (e.g. hot, gc, crash-loop) makes the run fail right after that
scenario's check passes, to exercise the failure dumps above.

ANDYSU_TEST_DROP_LOG=<event> (e.g. payload_started) makes the test payload skip that event's logcat line
once after `pm clear`, to exercise the fallback to the payload's events file (a lost log line must not
fail a run). --split-loop N repeats only the `split` install + first launch N times;
ANDYSU_TEST_STRESS=<k> adds k busy loops in the guest.
"""
import argparse
import json
import os
import queue
import re
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



def debug_keystore():
    """The Android debug keystore, created like the SDK does when missing (fresh CI runners)."""
    ks = os.path.expanduser("~/.android/debug.keystore")
    if not os.path.exists(ks):
        os.makedirs(os.path.dirname(ks), exist_ok=True)
        run(["keytool", "-genkeypair", "-keystore", ks, "-storepass", "android", "-keypass", "android",
             "-alias", "androiddebugkey", "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000",
             "-dname", "CN=Android Debug,O=Android,C=US"])
    return ks

def gradle(*args):
    return run([os.path.join(ANDROID, "gradlew"), "-q", "-p", ANDROID, "-Pandysu.abis=x86_64", *args], timeout=3600)


class Fail(Exception):
    pass


ANIM_SCALES = ["window_animation_scale", "transition_animation_scale", "animator_duration_scale"]
WATCHDOG_MS = 6000  # the test host's watchdog_ms
SLOW = 1.0  # --slow: multiplies every timeout (software-emulated devices)
CONTAINER = False  # --container: run emulators inside the andysu-emulator podman image
IGNORE_EVENT = os.environ.get("ANDYSU_TEST_IGNORE_EVENT", "")  # "event[:state[:release]]": drop the first such line once


def _matches_ignore(ev):
    """ANDYSU_TEST_IGNORE_EVENT simulates a host log line lost by logd (tests the state.json fallback)."""
    global IGNORE_EVENT
    name, _, rest = IGNORE_EVENT.partition(":")
    state, _, release = rest.partition(":")
    st = ev.get("status") or {}
    if ev.get("event") != name or (state and st.get("state") != state) or (release and st.get("release") != release):
        return False
    IGNORE_EVENT = ""
    return True


FORCE_FAIL = os.environ.get("ANDYSU_TEST_FORCE_FAIL", "")  # scenario name: fail after it (tests the dumps)
DROP_LOG = os.environ.get("ANDYSU_TEST_DROP_LOG", "")  # payload event name whose log line is dropped once
STAMP = time.strftime("%Y%m%d-%H%M%S")
REPORTS = os.path.join(ANDROID, "tests", "reports")


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
            "check_interval_minutes": 360, "watchdog_ms": WATCHDOG_MS, "max_boot_attempts": 2, "host_api": 1,
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
                              "--keystore", debug_keystore(), "--ks-pass", "pass:android",
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
        self.logpath = os.path.join(build.work, "server.log")  # `andysu serve` stderr: one line per request

    def reset(self):
        self.stop()
        self.site = tempfile.mkdtemp(prefix="site-", dir=self.b.work)
        os.makedirs(os.path.join(self.site, "stable"), exist_ok=True)
        self.serve()

    def serve(self, *extra):
        self.stop()
        with open(self.logpath, "ab") as lf:
            lf.write(f"--- {time.strftime('%H:%M:%S')} serve {self.site} {' '.join(extra)}\n".encode())
            lf.flush()
            self.srv = subprocess.Popen([ANDYSU, "serve", self.site, "--addr", f"127.0.0.1:{self.port}", *extra],
                                        stdout=subprocess.DEVNULL, stderr=lf)
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
        self.packages = {PKG}
        self._stop_focus = threading.Event()

    def adb(self, *args, check=True, timeout=120):
        return run([ADB, "-s", self.serial, *args], check=check, timeout=timeout * SLOW)

    def sh(self, cmd, check=True, timeout=120):
        return self.adb("shell", cmd, check=check, timeout=timeout)

    def start(self):
        if self.existing:
            self.adb("wait-for-device", timeout=60)
            self.restore_focus = self.sh("dumpsys window | grep mCurrentFocus", check=False).strip()
            self.restore_logcat = self._logcat_sizes()
            self.restore_anim = {k: self.sh(f"settings get global {k}", check=False).strip() for k in ANIM_SCALES}
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
        for k in ANIM_SCALES:
            self.sh(f"settings put global {k} 0", check=False)
        self.sh("input keyevent KEYCODE_WAKEUP", check=False)
        self.sh("wm dismiss-keyguard", check=False)
        self.sdk = int(self.sh("getprop ro.build.version.sdk").strip())
        self.page_size = int(self.sh("getconf PAGE_SIZE", check=False).strip() or "4096")
        log(f"[{self.api}] booted: SDK {self.sdk}, page size {self.page_size}")
        threading.Thread(target=self._focus_watch, daemon=True).start()

    def _logcat_sizes(self):
        """Buffer sizes in KiB per logcat buffer (`logcat -g`), to put back after watch() raised them."""
        sizes = {}
        for m in re.finditer(r"(\w+): ring buffer is\s+([\d.]+)\s*([KMG])i?B", self.sh("logcat -g", check=False), re.I):
            sizes[m.group(1)] = int(float(m.group(2)) * {"K": 1, "M": 1024, "G": 1024 * 1024}[m.group(3).upper()])
        return sizes

    def stop(self, keep=False):
        self._stop_focus.set()
        if self.logcat:
            self.logcat.kill()
        if self.existing:
            # Leave a shared device as we found it.
            for pkg in sorted(self.packages):
                self.adb("uninstall", pkg, check=False)
            self.adb("reverse", "--remove-all", check=False)
            for buf, kib in self.restore_logcat.items():
                self.adb("logcat", "-b", buf, "-G", f"{kib}K", check=False)
            for k, v in self.restore_anim.items():
                if v and v != "null":
                    self.sh(f"settings put global {k} {v}", check=False)
                else:
                    self.sh(f"settings delete global {k}", check=False)
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

    def dump_failure(self, remote=None, error=""):
        """Write everything needed to understand a failure into reports/ and return the file names."""
        os.makedirs(REPORTS, exist_ok=True)
        base = os.path.join(REPORTS, f"device-{STAMP}-api{self.api}")
        files = []

        def put(suffix, text):
            path = f"{base}-{suffix}"
            try:
                with open(path, "w", errors="replace") as f:
                    f.write(text)
                files.append(path)
            except Exception as e:
                log(f"[{self.api}] could not write {path}: {e}")

        def safe(fn, *a, **kw):
            try:
                return fn(*a, **kw)
            except Exception as e:
                return f"(failed: {e})\n"

        # a. events (nothing here can hang: pure data), b. app data, c. server log
        put("events.jsonl", "".join(json.dumps(e) + "\n" for e in list(self.events)))
        ad = [f"error: {error}\n"]
        for title, cmd in [("ls -lR no_backup/andysu", f"run-as {PKG} ls -lR no_backup/andysu"),
                           ("state.json", f"run-as {PKG} cat no_backup/andysu/state.json"),
                           ("state.lock", f"run-as {PKG} ls -l no_backup/andysu/state.lock"),
                           ("ps", "ps -A -o PID,PPID,STAT,WCHAN:20,NAME | grep -E 'andysu|PID'")]:
            ad.append(f"=== {title}\n{safe(self.sh, cmd, check=False, timeout=20)}\n")
        put("appdata.txt", "".join(ad))
        if remote is not None:
            try:
                with open(remote.logpath, errors="replace") as f:
                    put("server.log", f.read())
            except Exception as e:
                put("server.log", f"(no server log: {e})\n")

        # Java thread stacks: SIGQUIT makes ART dump every thread (the app is debuggable, so run-as may
        # signal it); the dump lands in logcat (and /data/anr). Then, only on emulators where `adb root`
        # works, read /data/anr and ask debuggerd for native stacks. Root comes last: it restarts adbd.
        th = []
        pids = safe(self.sh, f"pidof {PKG}", check=False, timeout=20).split()
        th.append(f"app pids: {pids}\n")
        marker = time.strftime("%m-%d %H:%M:%S")
        for pid in pids:
            th.append(f"=== kill -3 {pid}: {safe(self.sh, f'run-as {PKG} kill -3 {pid}', check=False, timeout=20)}\n")
        time.sleep(3)
        for pid in pids:
            th.append(f"=== debuggerd -j {pid} (non-root)\n{safe(self.sh, f'debuggerd -j {pid} 2>&1', check=False, timeout=30)}\n")
        root = False
        if self.serial.startswith("emulator-"):
            r = safe(self.adb, "root", check=False, timeout=30)
            root = "cannot run as root" not in r and "(failed" not in r and "production builds" not in r
            th.append(f"=== adb root: {r.strip()}\n")
            if root:
                safe(self.adb, "wait-for-device", timeout=60)
                time.sleep(2)
        if root:
            th.append("=== /data/anr\n" + safe(self.sh, "ls -l /data/anr", check=False, timeout=20) + "\n")
            th.append("=== newest /data/anr files\n" + safe(
                self.sh, "for f in $(ls -t /data/anr/* 2>/dev/null | head -3); do echo \"##### $f\"; cat $f; done",
                check=False, timeout=60) + "\n")
            for pid in pids:
                th.append(f"=== debuggerd -b {pid} (native, root)\n{safe(self.sh, f'debuggerd -b {pid} 2>&1', check=False, timeout=60)}\n")
        full = safe(self.adb, "logcat", "-d", "-v", "threadtime", check=False, timeout=120)
        put("logcat.txt", full)
        block = []  # the "----- pid N at ... -----" thread dump blocks that SIGQUIT wrote into logcat
        on = False
        for line in full.splitlines():
            if "----- pid " in line and " at " in line:
                on = True
            if on:
                block.append(line)
            if "----- end " in line:
                on = False
        th.append("=== SIGQUIT thread dump from logcat\n" + ("\n".join(block) if block else
                  "(none found in logcat; see /data/anr output above)") + "\n")
        put("threads.txt", "".join(th))
        if root:
            safe(self.adb, "unroot", check=False, timeout=30)
            safe(self.adb, "wait-for-device", timeout=60)
        return files

    # -- app control

    def fresh(self, pkg):
        """Uninstall `pkg` (a clean start); on a shared device (--device) it is uninstalled again in stop()."""
        self.adb("uninstall", pkg, check=False)
        self.packages.add(pkg)

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
        self.adb("logcat", "-G", "16M", check=False)  # keep the whole run for the failure dump
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
                    if IGNORE_EVENT and _matches_ignore(ev):
                        log(f"[{self.api}] ANDYSU_TEST_IGNORE_EVENT: dropping {json.dumps(ev)[:120]}")
                        continue
                    if ev.get("event") == "verify_retry":
                        # The payload VerifyError mitigation fired (see PROGRESS): rare, and invisible in a
                        # passing job unless it is printed here.
                        log(f"[{self.api}] WARNING: payload verification retried: {json.dumps(ev)[:300]}")
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
        # A payload event can vanish if liblog drops the write (logd busy). The payload also keeps its
        # latest emit of each event in a file (with its pid); accept that only for the app's live process.
        for ev in self.payload_file_events():
            if pred(ev):
                log(f"[{self.api}] WARNING: {what}: log line missing, accepted from the payload's events file "
                    f"(pid {ev.get('pid')})")
                ev = dict(ev, _from_file=True)
                self.events.append(ev)
                return ev
        tail = "\n".join(json.dumps(e)[:300] for e in self.events[-20:])
        raise Fail(f"timed out waiting for {what}\n{tail}")

    def payload_file_events(self):
        """The test payload's latest events from its app data, only those written by a live app process."""
        try:
            pids = set(self.sh(f"pidof {PKG}", check=False, timeout=20).split())
            data = json.loads(self.sh(f"run-as {PKG} cat files/andysu-last-events.json", check=False, timeout=20))
            return [e for e in data.values() if isinstance(e, dict) and str(e.get("pid")) in pids]
        except Exception:
            return []

    def payload(self, version, timeout=90):
        ev = self.wait(lambda e: e.get("event") == "payload_started" and e.get("version") == version,
                       f"payload {version} started", timeout)
        self.last_payload = ev
        return ev

    def event(self, name, timeout=60, **match):
        return self.wait(lambda e: e.get("event") == name and all(e.get(k) == v for k, v in match.items()),
                         f"event {name} {match}", timeout)

    def status(self, state, timeout=120):
        try:
            return self.wait(lambda e: e.get("event") == "update_status" and e["status"].get("state") == state,
                             f"update status {state}", timeout)
        except Fail:
            if state != "staged":
                raise
            # The host's own log line can be dropped too (liblog drops writes while logd is busy; seen
            # on CI: release 6.0 staged per state.json, its "staged" line missing from the full logcat).
            # Accept state.json only for the release the last observed update_status was fetching.
            want = next((e["status"].get("release") for e in reversed(self.events)
                         if e.get("event") == "update_status" and e["status"].get("release")), None)
            st = self.state()
            pend = (st.get("pending") or {}).get("id")
            if want and pend == want and "staged" in (st.get("last_event") or ""):
                log(f"[{self.api}] WARNING: update status staged: log line missing, {want} is pending in state.json")
                ev = {"event": "update_status", "status": {"state": "staged", "release": want}, "_from_state": True}
                self.events.append(ev)
                return ev
            raise


# ----------------------------------------------------------------------------- scenarios


def scenarios(d, b, r, results):
    def ok(name, detail=""):
        results.append({"api": d.api, "sdk": d.sdk, "scenario": name, "ok": True, "detail": detail})
        log(f"[{d.api}] ok  {name}  {detail}")
        if FORCE_FAIL == name:
            raise Fail(f"forced failure after scenario {name} (ANDYSU_TEST_FORCE_FAIL)")

    def count(name):
        return sum(1 for e in d.events if e.get("event") == name)

    d.install(b.apk)
    d.adb("reverse", f"tcp:{r.port}", f"tcp:{r.port}")
    d.clear()
    if DROP_LOG:  # make the payload skip that event's logcat line once, to exercise Device.payload_file_events
        d.sh(f"run-as {PKG} sh -c 'mkdir -p files && echo {DROP_LOG} > files/drop-log'")
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
    newest = open(r.manifest_path(), "rb").read(), open(r.manifest_path() + ".sig", "rb").read()
    d.broadcast("check")
    d.status("up_to_date")
    open(r.manifest_path(), "wb").write(good)
    open(r.manifest_path() + ".sig", "wb").write(good_sig)
    d.broadcast("check")
    e = d.status("error")
    if e["status"].get("code") != "sequence_rollback":
        raise Fail(f"replay: {e}")
    # Expired: re-sign the newest manifest (a fresh sequence) to expire right away, and check
    # once that instant has passed on the device's clock too.
    open(r.manifest_path(), "wb").write(newest[0])
    open(r.manifest_path() + ".sig", "wb").write(newest[1])
    r.sign("--expires-days", "0")
    time.sleep(2.5)
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
    since = len(d.events)
    r.publish("8.5")
    d.broadcast("check")
    d.status("staged")
    time.sleep(3 * SLOW)
    if any(e.get("event") in ("live_swapped", "relaunch") for e in d.events[since:]):
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

    # D30: a payload replaces the host's SIGSEGV handler. Default (crash_signals = payload): the host only
    # reports it. Opt-in (crash_signals = host): it re-installs its handler and chains the payload's.
    # (runs on the test host again, updated to 8.5 first; each rollback below returns to 9.2)
    d.adb("uninstall", PKG, check=False)
    d.install(b.apk)
    d.launch()
    d.event("live_swapped", to="8.5", timeout=120)
    d.payload("8.5")
    d.event("committed", version="8.5")
    # (a) default, handler that HANDLES a SIGSEGV and resumes (Mono): reported, never relaunched, commits
    inits = count("init")
    relaunches = count("relaunch")
    crashes = count("native_crash")
    r.publish("9.2")
    d.broadcast("check")
    d.payload("9.2")
    rep = d.event("crash_handler_replaced", timeout=30)
    if "SIGSEGV" not in rep.get("signals", []) or rep.get("chained") is not False:
        raise Fail(f"default mode must only report: {rep}")
    seg = d.event("segv_handled", timeout=30)
    if seg.get("handled") != 1:
        raise Fail(f"payload did not resume from its handled fault: {seg}")
    d.event("committed", version="9.2")
    if count("init") != inits or count("relaunch") != relaunches or count("native_crash") != crashes:
        raise Fail("a handled SIGSEGV relaunched or crashed the app in default mode")
    # (b) default, handler that hangs on a payload thread: not recorded by the host; the watchdog rolls back
    r.publish("9.1")
    d.broadcast("check")
    d.payload("9.1")
    rep = d.event("crash_handler_replaced", timeout=30)
    if rep.get("chained") is not False:
        raise Fail(f"default mode must not chain: {rep}")
    d.event("watchdog", timeout=60)
    rb = d.event("live_rollback", timeout=30)
    if "watchdog" not in rb.get("reason", ""):
        raise Fail(f"expected a watchdog rollback in default mode: {rb}")
    d.payload("9.2")
    if "9.1" not in d.state()["bad"]:
        raise Fail("9.1 not marked bad")
    if count("native_crash") != crashes:
        raise Fail("default mode must not record the crash itself")
    # (c) takeover: the host re-installs, writes the marker, relaunches immediately even though the
    # payload's handler (on the main thread) never returns
    d.broadcast("crashsignals", mode="host")
    d.event("crash_signals", mode="host")
    inits = count("init")
    r.publish("9.0")
    d.broadcast("check")
    d.payload("9.0")
    rep = d.event("crash_handler_replaced", timeout=30)
    if "SIGSEGV" not in rep.get("signals", []) or rep.get("chained") is not True:
        raise Fail(f"takeover mode did not re-install and chain: {rep}")
    d.event("native_crash", timeout=30)
    d.event("relaunch")
    launch = d.event("launch", timeout=60)
    if not any(e.get("event") == "rolled_back" and "native crash" in e.get("reason", "") for e in launch.get("events", [])):
        raise Fail(f"launch after the hung handler did not roll back: {launch}")
    d.payload("9.2")
    if count("init") - inits != 1:
        raise Fail("expected exactly one relaunch after the hung payload handler")
    if "9.0" not in d.state()["bad"]:
        raise Fail("9.0 not marked bad")
    d.broadcast("crashsignals")  # back to the configured default
    d.event("crash_signals", mode="payload")
    ok("crash-handler", "default: handled SIGSEGV resumes and commits 9.2 (no relaunch), hung payload-thread handler -> watchdog rollback, 9.1 bad; host: re-install + chain -> marker, relaunch -> 9.2, 9.0 bad")

    # D31: the trial watchdog counts foreground time only. 9.3 becomes healthy only after the app was stopped
    # and came back; the app goes HOME right after the swap and stays away longer than watchdog_ms.
    r.publish("9.3")
    d.broadcast("check")
    d.payload("9.3")
    d.home()
    paused = d.event("watchdog_paused", timeout=30)
    d.event("payload_stopped", timeout=30)
    since = len(d.events)
    time.sleep(WATCHDOG_MS / 1000 * 2 * SLOW)
    if any(e.get("event") in ("watchdog", "live_rollback", "rolled_back", "committed") for e in d.events[since:]):
        raise Fail(f"the trial ended while the app was in the background: {d.events[since:]}")
    d.launch()
    resumed = d.event("watchdog_resumed", timeout=30)
    d.event("payload_resumed", timeout=30)
    d.event("committed", version="9.3", timeout=30)
    if any(e.get("event") in ("watchdog", "live_rollback") for e in d.events[since:]):
        raise Fail("the watchdog fired although the app was away only in the background")
    if (d.state().get("current") or {}).get("id") != "9.3" or "9.3" in d.state()["bad"]:
        raise Fail(f"9.3 should be current and not bad: {d.state()}")
    ok("background-trial", f"9.3 trial: HOME right after the swap, away {WATCHDOG_MS * 2 // 1000} s (watchdog_ms {WATCHDOG_MS}); "
                           f"watchdog paused with {paused['remaining_ms']} ms left, resumed with {resumed['remaining_ms']} ms, committed, no rollback")

    # Host API level 3: update mode is a persisted per-install override. check_only never downloads
    # or stages; download_only stages but never applies; off cancels checks/jobs, including check-on-open.
    d.broadcast("mode", mode="check_only")
    d.event("update_mode", mode="check_only")
    r.publish("9.4")
    d.broadcast("check")
    d.status("available")
    st = d.state()
    if st.get("pending") or (st.get("current") or {}).get("id") != "9.3":
        raise Fail(f"check_only downloaded or staged 9.4: {st}")

    # Verify both the setting and check-on-open behavior survive a process restart.
    d.force_stop()
    mode_since = len(d.events)
    d.launch()
    d.event("launch")
    d.payload("9.3")
    d.status("available")
    d.broadcast("mode")
    mode_event = d.event("test_mode", mode="check_only")
    if not mode_event:
        raise Fail("check_only override did not survive process restart")

    d.broadcast("mode", mode="download_only")
    d.event("update_mode", mode="download_only")
    d.broadcast("check")
    d.status("staged")
    time.sleep(2 * SLOW)
    if any(e.get("event") in ("live_swapped", "relaunch") for e in d.events[mode_since:]):
        raise Fail("download_only automatically applied 9.4")
    d.force_stop()
    d.launch()
    cold = d.event("launch")
    if cold.get("in_trial"):
        raise Fail(f"download_only cold boot promoted 9.4: {cold}")
    d.payload("9.3")
    st = d.state()
    if (st.get("pending") or {}).get("id") != "9.4" or (st.get("current") or {}).get("id") != "9.3":
        raise Fail(f"download_only should leave 9.4 pending across a cold boot: {st}")

    d.broadcast("mode", mode="off")
    d.event("update_mode", mode="off")
    d.force_stop()
    off_since = len(d.events)
    d.launch()
    cold = d.event("launch")
    if cold.get("in_trial"):
        raise Fail(f"off mode cold boot promoted 9.4: {cold}")
    d.payload("9.3")
    time.sleep(2 * SLOW)
    if any(e.get("event") == "update_status" and e.get("status", {}).get("state") == "checking"
           for e in d.events[off_since:]):
        raise Fail("off mode still checked on app open")
    d.broadcast("mode")
    d.event("test_mode", mode="off")

    # An explicit restart still applies the pending id once while off; the marker is consumed by
    # the main-process launch and cannot promote a different later release.
    d.broadcast("restart")
    d.event("relaunch", reason="restart requested")
    cold = d.event("launch")
    if not cold.get("in_trial"):
        raise Fail(f"explicit restart did not promote pending 9.4: {cold}")
    d.payload("9.4")
    d.event("committed", version="9.4")

    # Clearing the override restores the configured auto mode.
    d.broadcast("mode", mode="null")
    d.event("update_mode", mode="auto")
    d.broadcast("mode")
    d.event("test_mode", mode="auto")
    ok("update-mode", "check_only reports without staging; download_only stages and preserves pending on cold boot; off persists and suppresses check-on-open/cold promotion; explicit restart applies once; clearing restores auto")

    # no dialogs
    bad_focus = [f for f in d.focus_log if any(x in f.lower() for x in
                 ["packageinstaller", "permissioncontroller", "grantpermissions", "application not responding",
                  "has stopped", "keeps stopping", "aerr", "anr"])]
    if bad_focus:
        raise Fail(f"a dialog appeared: {bad_focus}")
    ok("no-dialog", f"{len(d.focus_log)} focus changes observed, none to an installer, permission or crash dialog")


def split_loop(d, b, r, n, results):
    """--split-loop N: only the `split` install-and-first-launch, N times (flake hunting).

    Each round: uninstall, install the `andysu split` host, launch, and wait for either the built-in
    payload to start or the host's boot_error. Counts and prints the failures; raises Fail when any."""
    d.adb("reverse", f"tcp:{r.port}", f"tcp:{r.port}")
    r.reset()
    bad = []
    # ANDYSU_TEST_STRESS=<k>: k busy loops in the guest (CPU contention, like a loaded CI runner)
    for _ in range(int(os.environ.get("ANDYSU_TEST_STRESS", "0"))):
        subprocess.Popen([ADB, "-s", d.serial, "shell", "sh -c 'while :; do :; done'"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for i in range(n):
        d.adb("uninstall", PKG, check=False)
        d.install(b.split_apk)
        d.watch()
        d.launch()
        ev = d.wait(lambda e: e.get("event") in ("payload_started", "boot_error"), f"split round {i}", 60)
        if ev.get("event") == "boot_error":
            bad.append((i, ev.get("error", "")[:300]))
            log(f"[{d.api}] split round {i}/{n}: BOOT ERROR {ev.get('error', '')[:200]}")
        else:
            log(f"[{d.api}] split round {i}/{n}: ok")
        d.force_stop()
    results.append({"api": d.api, "sdk": d.sdk, "scenario": "split-loop", "ok": not bad,
                    "detail": f"{len(bad)} of {n} first launches failed"})
    log(f"[{d.api}] split-loop: {len(bad)} of {n} failed")
    if bad:
        raise Fail(f"split-loop: {len(bad)} of {n} first launches failed: {bad}")


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
    ap.add_argument("--split-loop", type=int, default=0, metavar="N",
                    help="only repeat the `split` install + first launch N times and count boot errors")
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
                   ("4.0", "crash_java"), ("5.0", "never_healthy"), ("6.0", "exit_on_start"),
                   ("9.0", "own_handler_crash"), ("9.1", "own_handler_thread"), ("9.2", "handled_segv"),
                   ("9.3", "healthy_after_resume"), ("9.4", "ok")]:
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
            if args.split_loop:
                split_loop(d, b, r, args.split_loop, results)
            else:
                scenarios(d, b, r, results)
        except Exception as e:
            try:
                files = d.dump_failure(r, str(e))
            except Exception as e2:
                files = [f"(dump failed: {e2})"]
            diag = d.diagnostics()
            names = [os.path.relpath(f, REPO) if f.startswith(REPO) else f for f in files]
            failures.append({"api": api, "error": str(e), "diagnostics": diag, "dump_files": names})
            log(f"[{api}] FAILED: {e}\n--- diagnostics ---\n{diag}\n--- full dumps (CI artifact) ---\n" + "\n".join(names))
        finally:
            r.stop()
            d.stop()

    os.makedirs(REPORTS, exist_ok=True)
    stamp = STAMP
    rep = {"when": stamp, "results": results, "failures": failures}
    with open(os.path.join(ANDROID, "tests", "reports", f"device-{stamp}.json"), "w") as f:
        json.dump(rep, f, indent=2)
    with open(os.path.join(ANDROID, "tests", "reports", f"device-{stamp}.md"), "w") as f:
        f.write(f"# Device test run {stamp}\n\n| API | SDK | scenario | detail |\n|---|---|---|---|\n")
        for x in results:
            f.write(f"| {x['api']} | {x['sdk']} | {x['scenario']} | {x['detail']} |\n")
        for x in failures:
            f.write(f"\n**FAILED on {x['api']}:**\n\n```\n{x['error'][:4000]}\n```\n")
            f.write("\nDumps: " + ", ".join(f"`{os.path.basename(n)}`" for n in x.get("dump_files", [])) + "\n")
    if not args.keep:
        shutil.rmtree(work, ignore_errors=True)
    log(f"{len(results)} checks passed, {len(failures)} API levels failed")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
