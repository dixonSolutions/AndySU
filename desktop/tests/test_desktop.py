#!/usr/bin/env python3
"""Step 4: the Linux desktop host live-swaps native payload generations and rolls back.

Drives the real host binary, the `andysu` CLI (pack/publish) and `andysu serve` (the remote):

  A. factory payload boots
  B. live update v1: state carried over, fresh copy of the renamed dependency, commit
  C. v2 start() fails       -> live rollback to v1 in the same process, same state
  D. v3 never healthy       -> watchdog -> live rollback
  E. v4 crashes in trial    -> signal handler writes the crash marker and relaunches once ->
                               the core rolls back -> v1 runs
  F. next-start apply: v5 staged, applied on the next launch, committed
  G. v6 crashes in start() on a cold boot -> one relaunch -> back on v5
  H. many live swaps in one process keep working, each with its own dependency copy
"""
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

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DESK = os.path.join(ROOT, "desktop")
ANDYSU = os.path.join(ROOT, "target", "debug", "andysu")
HOST = os.path.join(ROOT, "target", "desktop", "andysu-desktop-host")
APP = "com.example.desktop"


def run(*args, cwd=None):
    r = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit(f"FAILED: {' '.join(args)}\n{r.stdout}\n{r.stderr}")
    return r.stdout


def build_payload(d, version, behavior=0, factory=False):
    lib = os.path.join(d, "lib", "x86_64")
    os.makedirs(lib, exist_ok=True)
    ex = os.path.join(DESK, "examples", "counter")
    ld = ["-shared", "-fPIC", "-Wl,-z,max-page-size=16384"]
    dep_v = str(int(version.split(".")[0]))
    run("cc", *ld, "-Wl,-soname,libdep.so", f"-DDEP_VERSION={dep_v}", "-o", os.path.join(lib, "libdep.so"),
        os.path.join(ex, "dep.c"))
    extra = ["-Wl,-rpath,$ORIGIN"] if factory else []
    run("cc", *ld, "-Wl,-soname,libcounter.so", f"-I{DESK}/include", f'-DVERSION="{version}"',
        f"-DBEHAVIOR={behavior}", "-o", os.path.join(lib, "libcounter.so"), os.path.join(ex, "counter.c"),
        f"-L{lib}", "-ldep", *extra)
    with open(os.path.join(d, "andysu-payload.json"), "w") as f:
        json.dump({"schema": 1, "app": APP, "version": version, "host_api_min": 1,
                   "entry": {"native_lib": "libcounter", "symbol": "andysu_payload_v1"}}, f)


class Host:
    def __init__(self, t, url, live, apply=None):
        args = [HOST, "--root", t.dev, "--factory", t.factory, "--url", url, "--pub", t.pub, "--app", APP,
                "--watchdog-ms", "1500"]
        if live:
            args.append("--live")
        if apply:
            args += ["--apply", apply]
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  text=True, bufsize=1)
        self.q = queue.Queue()
        self.log = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                ev = {"event": "raw", "line": line}
            self.log.append(ev)
            self.q.put(ev)

    def send(self, cmd):
        self.p.stdin.write(cmd + "\n")
        self.p.stdin.flush()

    def wait(self, pred, what, timeout=20):
        end = time.time() + timeout
        while time.time() < end:
            try:
                ev = self.q.get(timeout=0.2)
            except queue.Empty:
                if self.p.poll() is not None and self.q.empty():
                    break
                continue
            if pred(ev):
                return ev
        tail = "\n".join(json.dumps(e) for e in self.log[-25:])
        raise SystemExit(f"FAILED waiting for {what}\n--- last events ---\n{tail}\n--- stderr ---\n"
                         f"{self.p.stderr.read() if self.p.poll() is not None else ''}")

    def payload(self, version, timeout=20):
        return self.wait(lambda e: e.get("event") == "payload" and e.get("version") == version,
                         f"payload {version}", timeout)

    def quit(self):
        self.send("quit")
        self.p.wait(timeout=10)


class T:
    pass


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def main():
    t = T()
    tmp = tempfile.mkdtemp(prefix="andysu-desktop-")
    t.dev, t.factory, site = os.path.join(tmp, "device"), os.path.join(tmp, "factory"), os.path.join(tmp, "site")
    keys = os.path.join(tmp, "keys")
    os.makedirs(keys)
    run(ANDYSU, "keygen", "--dir", keys)
    t.pub, key = os.path.join(keys, "andysu-public.pem"), os.path.join(keys, "andysu-root.key")
    build_payload(t.factory, "0.0", factory=True)

    port = free_port()
    os.makedirs(site, exist_ok=True)
    srv = subprocess.Popen([ANDYSU, "serve", site, "--addr", f"127.0.0.1:{port}"], stderr=subprocess.DEVNULL)
    url = f"http://127.0.0.1:{port}/stable/update.json"
    time.sleep(0.3)

    def publish(version, behavior=0):
        d = os.path.join(tmp, "p" + version)
        build_payload(d, version, behavior)
        run(ANDYSU, "pack", d, "--site", site, "--require-16k")
        run(ANDYSU, "publish", "--site", site, "--release", version, "--key", key)

    results = []

    def ok(name):
        results.append(name)
        print(f"ok  {name}", flush=True)

    try:
        h = Host(t, url, live=True)
        ev = h.payload("0.0")
        assert ev["dep_instances"] == 1
        ok("A factory payload boots")

        publish("1.0")
        time.sleep(0.5)
        h.send("check")
        h.wait(lambda e: e.get("event") == "live_swapped" and e["to"] == "1.0", "live swap to 1.0")
        ev = h.payload("1.0")
        assert ev["counter"] > 0, ev
        assert ev["dep_version"] == 1 and ev["dep_instances"] == 1, ev
        h.wait(lambda e: e.get("event") == "committed", "commit of 1.0")
        ok(f"B live update to 1.0, counter carried over ({ev['counter']}), own dependency copy, committed")

        publish("2.0", behavior=1)
        h.send("check")
        rb = h.wait(lambda e: e.get("event") == "live_rollback", "live rollback of 2.0")
        assert "start() returned 7" in rb["reason"], rb
        ev = h.payload("1.0")
        assert ev["counter"] > 0
        st = json.loads(run(ANDYSU, "client", "--root", t.dev, "state"))
        assert "2.0" in st["bad"], st
        ok("C 2.0 start() fails -> live rollback to 1.0 in-process, 2.0 marked bad")

        publish("3.0", behavior=2)
        h.send("check")
        h.payload("3.0")
        h.wait(lambda e: e.get("event") == "watchdog", "watchdog on 3.0")
        h.wait(lambda e: e.get("event") == "live_rollback", "live rollback of 3.0")
        h.payload("1.0")
        ok("D 3.0 never calls markHealthy -> watchdog -> live rollback")

        publish("4.0", behavior=3)
        pid = h.p.pid
        h.send("check")
        h.payload("4.0")
        boot = h.wait(lambda e: e.get("event") == "boot" and e.get("relaunches") == 1, "relaunch after crash")
        assert boot["pid"] == pid, "relaunch re-executes the same process"
        launch = h.wait(lambda e: e.get("event") == "launch", "launch after relaunch")
        evs = launch["result"]["ok"]["events"]
        assert any(e["event"] == "rolled_back" and "SIGSEGV" in e["reason"] for e in evs), evs
        h.payload("1.0")
        relaunches = sum(1 for e in h.log if e.get("event") == "boot") - 1
        assert relaunches == 1, relaunches
        ok("E 4.0 segfaults during trial -> crash marker -> exactly one relaunch -> rolled back to 1.0")
        h.quit()

        publish("5.0")
        h = Host(t, url, live=False)
        h.payload("1.0")
        h.send("check")
        h.wait(lambda e: e.get("event") == "check" and e["result"].get("ok", {}).get("staged") == "5.0", "5.0 staged")
        time.sleep(0.3)
        assert not any(e.get("event") == "payload" and e.get("version") == "5.0" for e in h.log)
        h.quit()
        h = Host(t, url, live=False)
        launch = h.wait(lambda e: e.get("event") == "launch", "launch")
        assert launch["result"]["ok"]["in_trial"] is True
        h.payload("5.0")
        h.wait(lambda e: e.get("event") == "committed", "commit of 5.0")
        h.quit()
        ok("F next-start apply: 5.0 staged while 1.0 runs, booted in trial on next launch, committed")

        publish("6.0", behavior=4)
        h = Host(t, url, live=False)
        h.payload("5.0")
        h.send("check")
        h.wait(lambda e: e.get("event") == "check" and e["result"].get("ok", {}).get("staged") == "6.0", "6.0 staged")
        h.quit()
        h = Host(t, url, live=False)
        h.wait(lambda e: e.get("event") == "boot" and e.get("relaunches") == 1, "relaunch after start() crash")
        h.payload("5.0")
        boots = sum(1 for e in h.log if e.get("event") == "boot")
        assert boots == 2, boots
        st = json.loads(run(ANDYSU, "client", "--root", t.dev, "state"))
        assert "6.0" in st["bad"] and st["current"]["id"] == "5.0", st
        h.quit()
        ok("G 6.0 crashes in start() on a cold boot -> one relaunch -> back on 5.0")

        h = Host(t, url, live=True)
        h.payload("5.0")
        for v in ["7.0", "8.0", "9.0"]:
            publish(v)
            h.send("check")
            ev = h.payload(v)
            assert ev["dep_instances"] == 1 and ev["dep_version"] == int(v[0]), ev
            h.wait(lambda e: e.get("event") == "committed", f"commit of {v}")
        swaps = [e for e in h.log if e.get("event") == "live_swapped"]
        assert swaps[-1]["live_swaps"] == 3, swaps
        h.quit()
        ok("H three live swaps in one process, each generation with its own dependency copy")
        publish("10.0")
        h = Host(t, url, live=False, apply="restart")
        h.payload("9.0")
        h.send("check")
        h.wait(lambda e: e.get("event") == "relaunch" and e.get("reason") == "applying update", "restart to apply")
        launch = h.wait(lambda e: e.get("event") == "launch", "launch after restart")
        assert launch["result"]["ok"]["in_trial"] is True, launch
        h.payload("10.0")
        h.wait(lambda e: e.get("event") == "committed", "commit of 10.0")
        h.quit()
        ok("I --apply restart: 10.0 staged, the host restarted itself once, 10.0 booted in trial and committed")
        gc = json.loads(run(ANDYSU, "client", "--root", t.dev, "gc"))
        ok(f"GC after the run removed {len(gc['removed_slots'])} slots, {gc['removed_blobs']} blobs")
    finally:
        srv.kill()
        shutil.rmtree(tmp, ignore_errors=True)
    print(f"\n{len(results)} desktop host scenarios passed")


if __name__ == "__main__":
    for p in (ANDYSU, HOST):
        if not os.path.exists(p):
            sys.exit(f"missing {p}; run `make -C desktop` and `cargo build -p andysu-cli`")
    main()
