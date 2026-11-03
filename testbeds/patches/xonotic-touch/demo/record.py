#!/usr/bin/env python3
"""README demo video: Xonotic Touch updating itself LIVE from a remote through AndySU.

Split screen. Left: real AndySU logs (the host's `AndySU-Event` / `AndySU` / `andysu-updater` logcat
lines, plus the captured, timestamped output of the real `andysu split / pack / publish / verify`
commands). Right: the game on an emulator, with the banner of patch 0008 (release, pid, process
uptime, live swap count). See ../LIVE_SWAP.md section 12.

Three stages, each usable alone:

  site     keys + `andysu split` of release A (a transparent host with A built in) + `andysu pack`/`publish`
           of release B into a site directory + `andysu verify --blobs`. Every command's output is
           captured with timestamps (<work>/cli.jsonl). The site is what gets published to GitHub Pages.
  record   boots an API 36 google_apis emulator (container mode), seeds the game data, installs the host,
           waits for the engine, records the screen (screenrecord) and the logcat, triggers the update
           check on cue, waits for live_swapped + committed, collects proof lines, pulls everything to
           <work>/raw/<label>/.
  render   turns a raw recording into the final files: left log panel drawn with PIL, composed with
           ffmpeg (H.264, yuv420p, faststart), plus a GIF preview and a poster PNG.

Examples (heavy jobs go through scripts/heavy.sh, CLAUDE.md):

  source scripts/env.sh
  # builds: scripts/heavy.sh testbeds/patches/xonotic-touch/build.sh --out testbeds/build/apks-demo \\
  #           --variants "1.2.9101:demo 1.2.9102:demo"
  python3 testbeds/patches/xonotic-touch/demo/record.py site        # real remote (GitHub Pages URL)
  scripts/heavy.sh python3 testbeds/patches/xonotic-touch/demo/record.py record
  python3 testbeds/patches/xonotic-touch/demo/record.py render --raw testbeds/build/xonotic-demo/raw/real

  # local dry run (separate host whose manifest_url is http://127.0.0.1:<port>, `andysu serve` + adb reverse):
  python3 .../record.py site --dry-run
  scripts/heavy.sh python3 .../record.py record --dry-run
  python3 .../record.py render --raw testbeds/build/xonotic-demo-dry/raw/dry-run --out testbeds/build/xonotic-demo-dry/out
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
sys.path.insert(0, os.path.join(REPO, "android", "tests"))

PKG = "io.github.dixonsolutions.xonotictouch"
JOB_ID = 0x414E4459  # com.andysu.host.Jobs.JOB_ID: `cmd jobscheduler run -f <pkg> <id>` runs a check in the live process
CHANNEL_URL = "https://dixonsolutions.github.io/AndySU/demo/xonotic/stable/update.json"
KEY_DIR = os.path.expanduser("~/.config/andysu/keys/xonotic-demo")
ANDYSU = os.path.join(REPO, "target", "debug", "andysu")
BUILDS = os.path.join(REPO, "testbeds", "build")
A_VERSION, B_VERSION = "1.2.9101", "1.2.9102"
FRAME_W, FRAME_H = 1920, 1080
# skip Xonotic Touch's first-run dialogs (welcome / profile, touch setup, terms of service): a clean main menu
ENGINE_ARGS = ("+set touch_profile_done 1 +set touch_setup_done 1 +set _termsofservice_accepted 2147483647 "
               "+set _cl_name AndySU +set cl_allow_uid2name 0")


def now():
    return time.time()


def ts(t):
    return time.strftime("%H:%M:%S", time.localtime(t)) + f".{int((t % 1) * 1000):03d}"


class Paths:
    def __init__(self, dry):
        self.dry = dry
        self.work = os.path.join(BUILDS, "xonotic-demo-dry" if dry else "xonotic-demo")
        self.site_root = os.path.join(BUILDS, "xonotic-demo-dry-site" if dry else "xonotic-demo-site")
        self.site = os.path.join(self.site_root, "demo", "xonotic")  # pack/publish --site: <site>/stable/update.json
        self.cli = os.path.join(self.work, "cli.jsonl")
        self.info = os.path.join(self.work, "site-info.json")


# ----------------------------------------------------------------------------- captured CLI sessions


class Cli:
    """Runs commands from BUILDS as the user would type them and captures every output line with a
    timestamp. The displayed command is the real argv (home directory shortened to ~)."""

    def __init__(self, path):
        self.path = path
        self.f = open(path, "a", buffering=1)

    def _put(self, kind, text, t=None):
        self.f.write(json.dumps({"t": t or now(), "src": "cli", "kind": kind, "text": text}) + "\n")

    def note(self, text):
        self._put("note", text)

    def run(self, argv, cwd=BUILDS, timeout=3600, show=None):
        shown = show or " ".join(("andysu" if a == ANDYSU else a.replace(os.path.expanduser("~"), "~")) for a in argv)
        self._put("cmd", "$ " + shown)
        p = subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
        out = []
        for line in p.stdout:
            line = line.rstrip("\n")
            out.append(line)
            self._put("out", line.replace(os.path.expanduser("~"), "~"))
        rc = p.wait(timeout=timeout)
        if rc != 0:
            self._put("err", f"(exit {rc})")
            raise SystemExit(f"command failed ({rc}): {shown}\n" + "\n".join(out[-20:]))
        return "\n".join(out)


def host_config(paths, pub_pem, dry, port):
    url = f"http://127.0.0.1:{port}/stable/update.json" if dry else CHANNEL_URL
    return {
        "app": PKG, "channel": "stable", "roots": [pub_pem], "manifest_url": url,
        # auto: a check downloads and stages what it finds. apply live: swap in the running process.
        "mode": "auto", "apply": "live",
        # the video triggers the check on cue (`cmd jobscheduler run -f <pkg> <job id>`), not at open
        "check_on_open": False,
        # watchdog: a live trial that never reports healthy is rolled back; the engine reports after ~3 s of frames
        "watchdog_ms": 45000, "auto_healthy_ms": 120000, "max_boot_attempts": 2,
        "host_api": 2,  # live native swap needs level 2 (min with what the AAR implements)
    }


def stage_site(args):
    P = Paths(args.dry_run)
    apk_a = os.path.abspath(args.apk_a or os.path.join(BUILDS, "apks-demo", f"XonoticTouch-{A_VERSION}-x86_64.apk"))
    apk_b = os.path.abspath(args.apk_b or os.path.join(BUILDS, "apks-demo", f"XonoticTouch-{B_VERSION}-x86_64.apk"))
    for p in (apk_a, apk_b, ANDYSU):
        if not os.path.exists(p):
            raise SystemExit(f"missing {p}")
    tpl = os.path.join(REPO, "android", "host-template", "build", "andysu", "andysu-host-template.zip")
    if not os.path.exists(tpl):
        raise SystemExit(f"missing {tpl}: run the :host-template:andysuHostTemplate Gradle task (via scripts/heavy.sh)")
    for d in (P.work, P.site_root):
        shutil.rmtree(d, ignore_errors=True)
    os.makedirs(P.work)
    os.makedirs(P.site)
    key = os.path.join(KEY_DIR, "xonotic-demo-root.key")
    pub = os.path.join(KEY_DIR, "xonotic-demo-public.pem")
    if not os.path.exists(key):
        os.makedirs(KEY_DIR, mode=0o700, exist_ok=True)
        subprocess.run([ANDYSU, "keygen", "--dir", KEY_DIR, "--name", "xonotic-demo"], check=True)
    port = args.port
    cfg = host_config(P, open(pub).read(), P.dry, port)
    cfg_path = os.path.join(P.work, "config.json")
    json.dump(cfg, open(cfg_path, "w"), indent=1)
    tpl_copy = os.path.join(P.work, "host-template.zip")  # a short path for the terminal panel
    shutil.copy(tpl, tpl_copy)
    tpl = tpl_copy
    cli = Cli(P.cli)
    rel = lambda p: os.path.relpath(p, BUILDS)  # noqa: E731
    ks = [os.path.expanduser("~/.android/debug.keystore")]
    cli.note("# the app, once: split release A into a frozen host APK (A built in) + its config")
    split_a = ["split", rel(apk_a), "--out", rel(os.path.join(P.work, "A")), "--transparent",
               "--host-template", rel(tpl), "--config", rel(cfg_path),
               "--keystore", ks[0], "--ks-pass", "pass:android", "--key-alias", "androiddebugkey"]
    if P.dry:
        split_a.append("--cleartext")  # plain-HTTP remote: local dry run only
    out = cli.run([ANDYSU, *split_a])
    ra = json.loads(out[out.index("{"):])
    cli.note("# later, on the publisher's machine: release B -> payload -> pack -> sign + publish")
    out = cli.run([ANDYSU, "split", rel(apk_b), "--out", rel(os.path.join(P.work, "B")), "--transparent"])
    rb = json.loads(out[out.index("{"):])
    cli.run([ANDYSU, "pack", rel(os.path.join(P.work, "B", "payload")), "--site", rel(P.site), "--no-rename"])
    cli.run([ANDYSU, "publish", "--site", rel(P.site), "--release", rb["version"], "--key", key,
             "--expires-days", "365"])  # a demo remote: the manifest should outlive the README video
    manifest = os.path.join(P.site, "stable", "update.json")
    cli.note("# what a client checks: signature, expiry, every blob's hash")
    cli.run([ANDYSU, "verify", rel(manifest), "--pub", pub, "--blobs"])
    info = {"A": ra["version"], "B": rb["version"], "host_apk": ra["host"]["apk"], "site": P.site, "site_root": P.site_root,
            "manifest_url": cfg["manifest_url"], "dry_run": P.dry, "port": port, "config": cfg_path, "split_a": ra}
    json.dump(info, open(P.info, "w"), indent=1)
    print(json.dumps({k: v for k, v in info.items() if k != "split_a"}, indent=1))
    return info


# ----------------------------------------------------------------------------- the recording (emulator)


def sh_quote(s):
    return "'" + s.replace("'", "'\\''") + "'"


def record(args):
    import device_tests as dt
    import testbed_tests as tt

    P = Paths(args.dry_run)
    if not os.path.exists(P.info):
        raise SystemExit(f"run `record.py site{' --dry-run' if P.dry else ''}` first ({P.info} missing)")
    info = json.load(open(P.info))
    label = args.label or ("dry-run" if P.dry else "real")
    raw = os.path.join(P.work, "raw", label)
    shutil.rmtree(raw, ignore_errors=True)
    os.makedirs(raw)
    A, B = info["A"], info["B"]
    host_apk = os.path.join(BUILDS, info["host_apk"])
    log = dt.log

    dt.CONTAINER = not args.device
    d = dt.Device(args.api, 5, existing=args.device)
    srv = None
    procs = []
    marks = {"label": label, "dry_run": P.dry, "A": A, "B": B, "manifest_url": info["manifest_url"], "api": args.api}
    adb = lambda *a, **k: d.adb(*a, **k)  # noqa: E731

    def device_now():
        return float(d.sh("date +%s.%N").strip())

    try:
        if P.dry:
            srv = subprocess.Popen([ANDYSU, "serve", info["site"], "--addr", f"127.0.0.1:{info['port']}"],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        d.start()
        d.fresh(PKG)
        d.install(host_apk)
        # The phone: 1280x720 landscape renders (and records) lighter than the Pixel 6's 2400x1080 under SwiftShader.
        d.sh("wm size 1280x720", check=False)
        d.sh("wm density 240", check=False)
        d.sh("settings put secure immersive_mode_confirmations confirmed", check=False)  # no "Viewing full screen" bubble
        d.sh("settings put global policy_control immersive.full=*", check=False)
        d.sh("settings put system accelerometer_rotation 0", check=False)
        d.sh("settings put system user_rotation 1", check=False)  # landscape
        # No network until the cue. Otherwise the host's own scheduled job (it runs soon after the first launch)
        # finds B, which is already published, and swaps it in before the recording starts.
        # (svc wifi/data disable is not enough on the emulator: it keeps its virtual network; airplane mode cuts it)
        d.sh("cmd connectivity airplane-mode enable", check=False)
        end = time.time() + 30
        while time.time() < end and re.search(r"Active default network: \d+", d.sh("dumpsys connectivity", check=False, timeout=30)):
            time.sleep(1)
        if re.search(r"Active default network: \d+", d.sh("dumpsys connectivity", check=False, timeout=30)):
            raise SystemExit("airplane mode did not take the emulator offline; the host would update before the cue")
        base = tt.seed_xonotic(d, PKG)
        # +developer 1 floods logcat with allocation lines; the demo does not need it
        d.sh(f"printf '%s\\n' {sh_quote(args.engine_args)} > {base}/xonotic.args", check=False)
        if P.dry:
            adb("reverse", f"tcp:{info['port']}", f"tcp:{info['port']}")
        d.sh("input keyevent KEYCODE_WAKEUP", check=False)

        # --- logcat (device clock, epoch). One process: the host's events, the host AAR, the updater's lines.
        adb("logcat", "-G", "16M", check=False)
        adb("logcat", "-c")
        lc = open(os.path.join(raw, "logcat.txt"), "w", buffering=1)
        procs.append(subprocess.Popen(
            [dt.ADB, "-s", d.serial, "logcat", "-v", "epoch", "AndySU-Event:I", "AndySU:V", "andysu-updater:V",
             "XonoticTouch:I", "*:S"], stdout=lc, stderr=subprocess.DEVNULL))
        d.watch()
        el = tt.EngineLog(d)  # engine/SDL lines (also what wait_event reads)
        # the focus log, to show that no installer / permission activity ever came up
        focus = []
        stop_focus = threading.Event()

        def focus_loop():
            while not stop_focus.is_set():
                try:
                    out = dt.run([dt.ADB, "-s", d.serial, "shell", "dumpsys window | grep -E 'mCurrentFocus'"], check=False, timeout=20)
                    line = out.strip()
                    if line and (not focus or focus[-1][1] != line):
                        focus.append((device_now_cache(), line))
                except Exception:
                    pass
                time.sleep(0.5)

        offset = [0.0]
        device_now_cache = lambda: time.time() + offset[0]  # noqa: E731
        offset[0] = device_now() - time.time()
        marks["device_clock_offset_s"] = offset[0]
        threading.Thread(target=focus_loop, daemon=True).start()

        # --- before: what the platform says about the installed host
        def pkginfo():
            out = d.sh(f"dumpsys package {PKG}", check=False)
            pick = lambda rx: (re.search(rx, out) or [None, ""])[1].strip()  # noqa: E731
            return {"versionName": pick(r"versionName=(\S+)"), "versionCode": pick(r"versionCode=(\d+)"),
                    "lastUpdateTime": pick(r"lastUpdateTime=(.+)"), "firstInstallTime": pick(r"firstInstallTime=(.+)"),
                    "codePath": pick(r"codePath=(\S+)"),
                    "appop_REQUEST_INSTALL_PACKAGES": d.sh(f"appops get {PKG} REQUEST_INSTALL_PACKAGES", check=False).strip()}

        def apk_sum():
            path = d.sh(f"pm path {PKG}", check=False).strip().replace("package:", "")
            return {"path": path, "sha256": d.sh(f"sha256sum {path}", check=False).split()[0] if path else ""}

        mark = el.mark()
        marks["t_launch"] = device_now()
        d.sh(f"monkey -p {PKG} -c android.intent.category.LAUNCHER 1", check=False)
        tt.wait_event(el, mark, "booted", "A booted", version=A)
        tt.wait_engine_start(el, mark, "A engine", 300, build_id=A, swapped=0)
        el.wait(r"AndySU-Event.*\"engine_frames\"", "A rendering", mark, timeout=300)
        pid = tt.wait_engine_start(el, mark, "A engine", 5, build_id=A, swapped=0)["pid"]
        marks["pid"] = pid
        marks["before"] = {"pkg": pkginfo(), "apk": apk_sum()}
        marks["focus_before"] = d.sh("dumpsys window | grep -E 'mCurrentFocus|mFocusedApp'", check=False).strip()
        log(f"A is up, pid {pid}: {marks['before']}")
        # game on screen first? a few seconds in the menu are enough; the banner shows the release.
        time.sleep(args.settle)

        # --- screenrecord (<= 170 s) and the clock reading in one device command
        size = args.size
        rec = (f"date +%s.%N; exec screenrecord --time-limit {args.limit} --bit-rate {args.bitrate} --size {size} "
               f"/sdcard/andysu-demo.mp4")
        p = subprocess.Popen([dt.ADB, "-s", d.serial, "shell", rec], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        procs.append(p)
        marks["t_record_start"] = float(p.stdout.readline().strip())
        log(f"screenrecord started at device time {ts(marks['t_record_start'])}")
        time.sleep(args.pre)  # video of A before the cue: the banner, pid, uptime

        # --- cue: the update check (a real check in the live process, exactly what the periodic job does)
        mark = el.mark()
        marks["t_cue"] = device_now()
        d.sh("cmd connectivity airplane-mode disable", check=False)
        d.sh("svc wifi enable", check=False)
        d.sh("svc data enable", check=False)
        end = time.time() + 90
        while time.time() < end:  # wait for a default network
            if re.search(r"Active default network: \d+", d.sh("dumpsys connectivity", check=False, timeout=30)):
                break
            time.sleep(1)
        marks["t_network"] = device_now()
        # (the emulator image has no curl; reaching the remote is shown by the update_status events that follow)
        d.sh(f"cmd jobscheduler run -f {PKG} {JOB_ID}", check=False)
        tt.wait_event(el, mark, "apply_decision", "decision for B", timeout=180)
        tt.wait_event(el, mark, "live_swap_requested", "live swap requested", timeout=120, to=B)
        tt.wait_event(el, mark, "live_swapped", "host sees the new engine started", timeout=240)
        _, com = tt.wait_event(el, mark, "committed", "markHealthy commits B", timeout=240, version=B)
        marks["t_committed"] = device_now()
        el.wait(r"AndySU-Event.*\"engine_frames\".*" + re.escape(B), "B rendering", mark, timeout=60)
        log(f"B is live and committed at {ts(marks['t_committed'])}: {json.dumps(com)[:200]}")
        time.sleep(args.hold)  # hold on B: banner, same pid, process uptime still counting

        # --- is the recorder still alive? (a stalled recorder would leave the video short of the log)
        marks["screenrecord_alive"] = d.sh("pidof screenrecord", check=False).strip()
        with open(os.path.join(raw, "screencap-after.png"), "wb") as f:
            subprocess.run([dt.ADB, "-s", d.serial, "exec-out", "screencap", "-p"], stdout=f, timeout=60)

        # --- proof lines
        marks["after"] = {"pkg": pkginfo(), "apk": apk_sum()}
        pids = d.sh(f"pidof {PKG}", check=False).split()
        marks["pids_after"] = pids
        marks["same_pid"] = pids == [str(pid)]
        marks["focus_after"] = d.sh("dumpsys window | grep -E 'mCurrentFocus|mFocusedApp'", check=False).strip()
        marks["state_json"] = d.sh(f"run-as {PKG} cat no_backup/andysu/state.json", check=False).strip()
        stop_focus.set()
        marks["focus_log"] = [{"t": t, "line": line} for t, line in focus]
        bad = [f for f in focus if re.search(r"packageinstaller|ManageAppExternalSources|InstallStart|permissioncontroller|GrantPermissions", f[1], re.I)]
        marks["installer_or_permission_focus"] = [f[1] for f in bad]
        marks["t_end"] = device_now()
        log(f"proof: same pid {marks['same_pid']}, installer/permission focus: {len(bad)}")
        json.dump(marks, open(os.path.join(raw, "marks.json"), "w"), indent=1)
        # let screenrecord end on its own (time limit) or stop it cleanly
        d.sh("pkill -INT screenrecord", check=False)
        time.sleep(3)
        adb("pull", "/sdcard/andysu-demo.mp4", os.path.join(raw, "screen.mp4"), timeout=300)
        el.stop()
        with open(os.path.join(raw, "engine-log.txt"), "w") as f:
            f.write("\n".join(el.lines) + "\n")
        shutil.copy(P.cli, os.path.join(raw, "cli.jsonl"))
        log(f"raw recording in {raw}")
    finally:
        for p in procs:
            try:
                p.kill()
            except Exception:
                pass
        if srv:
            srv.kill()
        if not args.keep:
            d.stop()
    return raw


# ----------------------------------------------------------------------------- the log panel (PIL) + composition

MONO = "/usr/share/fonts/adobe-source-code-pro-fonts/SourceCodePro-Regular.otf"
MONO_B = "/usr/share/fonts/adobe-source-code-pro-fonts/SourceCodePro-Semibold.otf"
SANS = "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf"

BG = (13, 17, 23)
PANEL = (22, 27, 34)
FG = (201, 209, 217)
DIM = (125, 133, 144)
CMD = (121, 192, 255)
GREEN = (63, 185, 80)
KEYBG = (24, 56, 36)
BLUE = (64, 153, 255)
ORANGE = (255, 166, 87)
PANEL_X, PANEL_W = 0, 820
CAP_H = 84
VID_X, VID_W, VID_H = 820, 1060, 596  # the phone video: 1280x720 scaled (20 px gutters)
VID_Y = 150
FSZ, LH = 17, 22
# events worth a highlight, and the ones that are only noise on a log panel
KEY_EVENTS = {"live_swap_requested", "live_swapped", "committed"}
SKIP_EVENTS = {"engine_frames", "proc_stats", "loader_engine_loaded", "engine_entry", "engine_state_restored", "init", "launch"}


def _font(path, size, variation=None):
    from PIL import ImageFont
    f = ImageFont.truetype(path, size)
    if variation:
        try:
            f.set_variation_by_name(variation)
        except Exception:
            f = ImageFont.truetype(MONO_B, size)
    return f


LOGCAT_RX = re.compile(r"^\s*(\d+\.\d+)\s+(\d+)\s+(\d+)\s+([VDIWEF])\s+(.+?)\s*:\s(.*)$")


def parse_logcat(path):
    """-> [(epoch, tag, text, parsed_event_or_None)]"""
    out = []
    for line in open(path, errors="replace"):
        m = LOGCAT_RX.match(line.rstrip("\n"))
        if not m:
            continue
        t, _pid, _tid, lvl, tag, text = m.groups()
        ev = None
        if tag == "AndySU-Event" and text.startswith("{"):
            try:
                ev = json.loads(text)
            except json.JSONDecodeError:
                pass
        out.append((float(t), tag, text, ev))
    return out


def ev_is(ev, name, **kw):
    return ev and ev.get("event") == name and all(ev.get(k) == v for k, v in kw.items())


def ev_text(ev):
    """`event {rest as compact JSON}`: the event name first, so a wrapped line still shows what it is."""
    rest = {k: v for k, v in ev.items() if k != "event"}
    if isinstance(rest.get("status"), dict):  # update_status: drop the fields that are constant noise here
        rest["status"] = {k: v for k, v in rest["status"].items()
                          if k not in ("already_staged", "hot_only", "mandatory", "notes_url", "not_modified", "sequence")}
    if ev["event"] == "committed":  # the full state object is long; the release and the result say it
        rest = {k: rest[k] for k in ("version", "result") if k in rest}
    for k in ("lib", "maps"):
        if isinstance(rest.get(k), str) and len(rest[k]) > 40:
            rest[k] = "..." + rest[k][-30:]
    return f"{ev['event']}  " + json.dumps(rest, separators=(",", ":"))


def digest_cli(rows):
    """The captured CLI output, with long JSON/warning blocks reduced to their telling lines ('[... n lines]'
    marks what was left out; every kept line is verbatim)."""
    out, cmd = [], ""
    i = 0
    while i < len(rows):
        r = rows[i]
        if r["kind"] in ("cmd", "note"):
            cmd = r["text"]
            out.append(r)
            i += 1
            continue
        block = []
        while i < len(rows) and rows[i]["kind"] not in ("cmd", "note"):
            block.append(rows[i])
            i += 1
        keep = block
        if " split " in cmd:
            want = re.compile(r'"(version|signed|apk|payload)"|"android.permission')
            keep = [b for b in block if want.search(b["text"])]
        elif " pack " in cmd:
            want = re.compile(r'^\s*"(release|sequence|components)"|^warning: .*libmain')
            keep = [b for b in block if want.search(b["text"])]
        if len(keep) < len(block):
            keep = keep + [dict(block[-1], text=f"[... {len(block) - len(keep)} more lines of the command's output]", kind="note")]
        out.extend(keep)
    return out


def wrap(text, cols, maxlines=3):
    lines = []
    while text and len(lines) < maxlines:
        lines.append(text[:cols])
        text = "  " + text[cols:] if text[cols:] else ""
    if text:
        lines[-1] = lines[-1][:cols - 3] + "..."
    return lines


def build_timeline(raw, offset):
    """-> (items, phases, info): items = [(t_video, [(text, colour, highlight)])], phases = [(t_video, caption, sub)]"""
    marks = json.load(open(os.path.join(raw, "marks.json")))
    logs = parse_logcat(os.path.join(raw, "logcat.txt"))
    cli = digest_cli([json.loads(l) for l in open(os.path.join(raw, "cli.jsonl"))])
    t0 = marks["t_record_start"] + offset
    tv = lambda t: t - t0  # noqa: E731
    cols = (PANEL_W - 40) // 10  # advance of the mono font is ~0.6 em
    items = []

    def first(name, after, **kw):
        for t, tag, text, ev in logs:
            if t >= after and ev_is(ev, name, **kw):
                return t
        return None

    cue = marks["t_cue"]
    t_checking = cue
    t_req = first("live_swap_requested", cue) or cue + 10
    t_swapped = first("live_swapped", t_req) or t_req + 20
    t_commit = first("committed", t_swapped) or t_swapped + 10
    # phone side (device clock)
    for t, tag, text, ev in logs:
        if ev is not None:
            if ev.get("event") in SKIP_EVENTS:
                continue
            body = ev_text(ev)
            key = (ev["event"] in KEY_EVENTS or (ev["event"] == "update_status" and ev["status"].get("state") == "staged")
                   or (ev["event"] == "engine_start" and ev.get("swapped") == 1))
        else:
            if tag == "XonoticTouch" or tag.startswith("andysu") or tag == "AndySU":
                body, key = re.sub(r"(accepted): .*", r"\1", text), False
            else:
                continue
        label = f"{ts(t)[:-1]} {tag[:12]:<12} "
        text_lines = wrap(label + body, cols)
        items.append((max(tv(t), 0.0), [(l, GREEN if key else FG, key) for l in text_lines]))
    # publisher side: the captured CLI output, replayed from the start of the video in two groups
    t_pub = None
    seq = []
    for r in cli:
        seq.append(r)
    t_cli = 0.6
    group2_started = None
    cur = t_cli
    for r in seq:
        if r["kind"] == "cmd":
            cur += 0.7
            if " pack " in r["text"] and group2_started is None:
                cur = max(cur, 4.0)  # the second group (what the publisher does) starts after the "installed once" part
                group2_started = cur
        elif r["kind"] == "note":
            cur += 0.25
        else:
            cur += 0.07
        col = {"cmd": CMD, "note": DIM, "out": FG, "err": (248, 81, 73)}[r["kind"]]
        stamp = time.strftime("%H:%M:%S", time.localtime(r["t"]))
        pre = f"{stamp} " if r["kind"] in ("cmd", "out") else ""
        items.append((cur, [(l, col, False) for l in wrap(pre + r["text"], cols, 2 if r["kind"] != "cmd" else 3)]))
    cli_end = cur
    group2_started = group2_started or 4.0
    # proof lines
    proof = proof_lines(marks)
    tp = tv(t_commit) + 2.5
    items.append((tp - 0.4, [("", FG, False), ("-- proof (adb, after the swap) " + "-" * 40, DIM, False)]))
    for l in proof:
        tp += 0.5
        items.append((tp, [(x, GREEN if l.startswith("OK ") else FG, False) for x in wrap(l, cols, 2)]))
    items.sort(key=lambda x: x[0])
    phases = [
        (0.0, "Installed once", "the AndySU host APK, release " + marks["A"] + " built in"),
        (group2_started, "Update published to GitHub Pages", "signed with the developer's key"),
        (tv(cue), "Downloading in the background", "the app keeps running"),
        (tv(t_req), "Swapped live: same process, same APK", "no restart"),
        (tv(t_commit), "Committed", "no install prompt, no unknown-apps flow"),
    ]
    info = {"t_cue": tv(cue), "t_req": tv(t_req), "t_swapped": tv(t_swapped), "t_commit": tv(t_commit), "cli_end": cli_end,
            "marks": marks}
    return items, phases, info


def proof_lines(m):
    b, a = m.get("before", {}), m.get("after", {})
    L = []
    pb, pa = b.get("pkg", {}), a.get("pkg", {})
    L.append(f"{'OK' if m.get('same_pid') else 'FAIL'} pidof {PKG}: {' '.join(m.get('pids_after', []))} (was {m.get('pid')})")
    L.append(f"{'OK' if pb.get('versionName') == pa.get('versionName') else 'FAIL'} dumpsys package versionName {pb.get('versionName')} -> {pa.get('versionName')}"
             " (the installed APK's version)")
    L.append(f"{'OK' if pb.get('lastUpdateTime') == pa.get('lastUpdateTime') else 'FAIL'} lastUpdateTime {pb.get('lastUpdateTime')} -> {pa.get('lastUpdateTime')}")
    sb, sa = (b.get("apk") or {}).get("sha256", ""), (a.get("apk") or {}).get("sha256", "")
    L.append(f"{'OK' if sb and sb == sa else 'FAIL'} sha256 of base.apk {sb[:12]}.. -> {sa[:12]}.. (same file)")
    bad = m.get("installer_or_permission_focus", [])
    L.append(f"{'OK' if not bad else 'FAIL'} window focus log: {'no installer, no permission or unknown-sources screen' if not bad else bad[0]}")
    op = pa.get("appop_REQUEST_INSTALL_PACKAGES")
    if op:
        L.append(f"appops REQUEST_INSTALL_PACKAGES: {op.splitlines()[0]}")
    return L


def draw_text(d, xy, text, font, fill):
    d.text(xy, text, font=font, fill=fill)


def render(args):
    from PIL import Image, ImageDraw
    raw = os.path.abspath(args.raw)
    out_dir = os.path.abspath(args.out or os.path.join(REPO, "docs", "media"))
    os.makedirs(out_dir, exist_ok=True)
    work = os.path.join(raw, "render")
    shutil.rmtree(work, ignore_errors=True)
    os.makedirs(os.path.join(work, "frames"))
    prefix = "andysu-xonotic-live-update" + ("-dry-run" if "dry" in os.path.basename(raw) else "")
    screen = os.path.join(raw, "screen.mp4")
    dur = float(subprocess.check_output(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", screen]).decode())
    items, phases, info = build_timeline(raw, args.video_offset)
    m = info["marks"]
    print(f"video {dur:.1f} s; cue {info['t_cue']:.1f}, swap requested {info['t_req']:.1f}, live_swapped {info['t_swapped']:.1f}, committed {info['t_commit']:.1f}")

    fm, fb = _font(MONO, FSZ), _font(MONO_B, FSZ)
    cap_big, cap_sub = _font(SANS, 40, "Bold"), _font(SANS, 24, "Regular")
    lab, small = _font(SANS, 22, "SemiBold"), _font(SANS, 24, "Regular")
    bullets = [("Installed once", "the APK never changes"), ("The remote installs updates", "from GitHub Pages"),
               ("Live swap, no restart", "same process, same pid"), ("No install prompt", "no unknown-apps flow, same APK")]
    bullet_at = [0.0, phases[1][0], phases[3][0], phases[4][0]]

    # static chrome
    base = Image.new("RGB", (FRAME_W, FRAME_H), BG)
    bd = ImageDraw.Draw(base)
    bd.rectangle([PANEL_X, CAP_H, PANEL_X + PANEL_W - 1, FRAME_H], fill=PANEL)
    bd.text((PANEL_X + 20, CAP_H + 12), "AndySU logs (real: logcat AndySU-Event + andysu CLI)", font=lab, fill=DIM)
    bd.text((VID_X + 20, CAP_H + 14), f"Xonotic Touch on an Android emulator (API {m.get('api')})", font=lab, fill=DIM)
    bd.rectangle([VID_X + 20 - 2, VID_Y - 2, VID_X + 20 + VID_W + 1, VID_Y + VID_H + 1], outline=(48, 54, 61), width=2)
    VX = VID_X + 20
    top = CAP_H + 50
    bottom = FRAME_H - 14
    nlines = (bottom - top) // LH

    # lines -> frames
    times = sorted({round(t, 2) for t, _ in items} | {0.0, round(dur, 2)} | {round(p[0], 2) for p in phases}
                   | {round(x, 2) for x in bullet_at})
    times = [t for t in times if t <= dur]
    flat = []  # (t, text, colour, key)
    for t, lines in items:
        for (txt, col, key) in lines:
            flat.append((t, txt, col, key))
    listing = []
    for i, t in enumerate(times):
        shown = [x for x in flat if x[0] <= t + 1e-6]
        shown = shown[-nlines:]
        img = base.copy()
        d = ImageDraw.Draw(img)
        y = top
        for (_, txt, col, key) in shown:
            if key:
                d.rectangle([PANEL_X + 8, y - 1, PANEL_X + PANEL_W - 10, y + LH - 2], fill=KEYBG)
            d.text((PANEL_X + 20, y), txt, font=fb if key else fm, fill=col)
            y += LH
        # caption bar
        cap = [p for p in phases if p[0] <= t + 1e-6][-1]
        accent = BLUE if cap is phases[0] or cap is phases[1] else (ORANGE if cap is phases[2] else GREEN)
        d.rectangle([0, 0, FRAME_W, CAP_H - 1], fill=(1, 4, 9))
        d.rectangle([0, CAP_H - 4, FRAME_W, CAP_H - 1], fill=accent)
        d.text((24, 16), cap[1], font=cap_big, fill=accent)
        wcap = d.textlength(cap[1], font=cap_big)
        d.text((24 + wcap + 24, 34), cap[2], font=cap_sub, fill=DIM)
        # the pitch, below the video
        yb = VID_Y + VID_H + 28
        for (head, sub), at in zip(bullets, bullet_at):
            on = t + 1e-6 >= at
            d.ellipse([VX, yb + 8, VX + 18, yb + 26], fill=GREEN if on else (48, 54, 61))
            d.text((VX + 34, yb), head, font=small, fill=FG if on else (72, 78, 86))
            d.text((VX + 34 + d.textlength(head + "  ", font=small), yb), sub, font=small, fill=DIM if on else (60, 66, 74))
            yb += 38
        fn = os.path.join(work, "frames", f"f{i:04d}.png")
        img.save(fn)
        nxt = times[i + 1] if i + 1 < len(times) else dur
        listing.append((fn, max(nxt - t, 0.04)))
    lst = os.path.join(work, "frames.txt")
    with open(lst, "w") as f:
        for fn, dd in listing:
            f.write(f"file '{fn}'\nduration {dd:.3f}\n")
        f.write(f"file '{listing[-1][0]}'\n")
    print(f"{len(listing)} panel frames")

    mp4 = os.path.join(out_dir, prefix + ".mp4")
    fc = (f"[0:v]fps=15,format=rgb24[bg];[1:v]fps=15,scale={VID_W}:{VID_H}:flags=lanczos[fg];"
          f"[bg][fg]overlay={VX}:{VID_Y}:shortest=1,format=yuv420p[v]")
    ff = ["ffmpeg", "-y", "-v", "error", "-f", "concat", "-safe", "0", "-i", lst, "-i", screen,
          "-filter_complex", fc, "-map", "[v]", "-t", f"{dur:.2f}", "-c:v", "libx264", "-preset", "slow", "-crf", str(args.crf),
          "-maxrate", args.maxrate, "-bufsize", "6M", "-pix_fmt", "yuv420p", "-movflags", "+faststart", "-an", mp4]
    subprocess.run(ff, check=True)
    print(f"{mp4}: {os.path.getsize(mp4) / 1e6:.2f} MB")

    # poster: a moment after the commit, banner B and the proof lines on screen
    poster = os.path.join(out_dir, prefix + "-poster.png")
    tpost = min(info["t_commit"] + 7, dur - 1)
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-ss", f"{tpost:.2f}", "-i", mp4, "-frames:v", "1", poster], check=True)
    # gif: from just before the swap request to the commit (+4 s), at most `gif_secs`
    g0 = max(info["t_req"] - 3, 0)
    g1 = min(info["t_commit"] + 4, g0 + args.gif_secs, dur)
    gif = os.path.join(out_dir, prefix + ".gif")
    pal = os.path.join(work, "pal.png")
    vf = f"fps={args.gif_fps},scale={args.gif_width}:-1:flags=lanczos"
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-ss", f"{g0:.2f}", "-t", f"{g1 - g0:.2f}", "-i", mp4, "-vf", vf + ",palettegen=max_colors=96:stats_mode=diff", pal], check=True)
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-ss", f"{g0:.2f}", "-t", f"{g1 - g0:.2f}", "-i", mp4, "-i", pal, "-lavfi",
                    vf + "[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle", gif], check=True)
    print(f"{gif}: {os.path.getsize(gif) / 1e6:.2f} MB ({g1 - g0:.1f} s from {g0:.1f} s)")
    print(f"{poster}: {os.path.getsize(poster) / 1e3:.0f} kB (video frame at {tpost:.1f} s)")
    json.dump({"mp4": mp4, "gif": gif, "poster": poster, "duration": dur, "gif_window": [g0, g1], **{k: v for k, v in info.items() if k != "marks"}},
              open(os.path.join(work, "render.json"), "w"), indent=1)
    return mp4


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("site", "record", "render", "all"):
        s = sub.add_parser(name)
        s.add_argument("--dry-run", action="store_true", help="local remote (andysu serve + adb reverse), own host and work dir")
        s.add_argument("--port", type=int, default=18787, help="dry run: the port of `andysu serve`")
        s.add_argument("--apk-a")
        s.add_argument("--apk-b")
        s.add_argument("--api", default="36")
        s.add_argument("--device", help="an existing device serial instead of booting the emulator (not for the real recording)")
        s.add_argument("--keep", action="store_true", help="leave the emulator running")
        s.add_argument("--label")
        s.add_argument("--settle", type=float, default=6, help="seconds after the engine renders, before screenrecord starts")
        s.add_argument("--pre", type=float, default=18, help="seconds of video on release A before the cue")
        s.add_argument("--hold", type=float, default=20, help="seconds to hold on B after committed")
        s.add_argument("--limit", type=int, default=170, help="screenrecord --time-limit")
        s.add_argument("--bitrate", type=int, default=4_000_000)
        s.add_argument("--size", default="1280x720")
        s.add_argument("--engine-args", default=ENGINE_ARGS, help="the engine's command line (xonotic.args); the default skips the first-run dialogs")
        s.add_argument("--raw", help="render: the raw recording directory")
        s.add_argument("--out", help="render: output directory (default docs/media)")
        s.add_argument("--video-offset", type=float, default=0.6, help="render: seconds from the screenrecord command to its first frame")
        s.add_argument("--crf", type=int, default=27)
        s.add_argument("--maxrate", default="1400k")
        s.add_argument("--gif-width", type=int, default=1000)
        s.add_argument("--gif-fps", type=int, default=8)
        s.add_argument("--gif-secs", type=float, default=20)
    args = ap.parse_args()
    if args.cmd == "site":
        stage_site(args)
    elif args.cmd == "record":
        record(args)
    elif args.cmd == "render":
        render(args)
    else:
        stage_site(args)
        raw = record(args)
        args.raw = raw
        render(args)


if __name__ == "__main__":
    main()
