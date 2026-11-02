#!/usr/bin/env python3
"""Makes the signed site match channels.toml.

    reconcile.py --site site --key signing.key [--cert cert.json] [--andysu andysu] [--resign]

For every channel in channels.toml:
  - release not in the channel yet:  `andysu publish` (with its rollout, if set)
  - release older than what the channel offers now: `andysu rollback` (republished under a new
    sequence, as <id>-r<n>, so every client moves forward onto it)
  - only the rollout changed:         `andysu sign --rollout`
  - --resign (scheduled):             `andysu sign` to push `expires` forward
Prints one line per action; prints nothing when the site already matches.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tomllib


def manifest(site, channel):
    p = os.path.join(site, channel, "update.json")
    if not os.path.exists(p):
        return None
    with open(p) as f:
        return json.load(f)


def known_sequence(site, release):
    p = os.path.join(site, "releases", f"{release}.json")
    if not os.path.exists(p):
        sys.exit(f"release {release} is not in the site; ingest it first")
    with open(p) as f:
        return json.load(f)["sequence"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--site", required=True)
    ap.add_argument("--config", default="channels.toml")
    ap.add_argument("--key", required=True)
    ap.add_argument("--cert")
    ap.add_argument("--andysu", default="andysu")
    ap.add_argument("--resign", action="store_true")
    a = ap.parse_args()
    with open(a.config, "rb") as f:
        cfg = tomllib.load(f)
    sign = ["--key", a.key] + (["--cert", a.cert] if a.cert else [])
    expires = str(cfg.get("expires_days", 30))

    def andysu(*args):
        subprocess.run([a.andysu, *args, "--site", a.site, *sign, "--expires-days", expires], check=True,
                       stdout=subprocess.DEVNULL)

    for channel, want in cfg.get("channels", {}).items():
        release = want.get("release")
        if not release:
            continue
        rollout = int(want.get("rollout", 100))
        m = manifest(a.site, channel)
        releases = m["releases"] if m else []
        top = max(releases, key=lambda r: r["sequence"]) if releases else None
        # A rollback republishes <id> as <id>-r<n>: that still is the release asked for.
        top_base = re.sub(r"-r\d+$", "", top["id"]) if top else None
        rollout_now = (top.get("rollout") or {}).get("percent", 100) if top else None
        if top_base == release:
            if rollout_now != rollout:
                andysu("sign", "--channel", channel, "--rollout", f"{top['id']}={rollout}")
                print(f"{channel}: {top['id']} rollout {rollout_now}% -> {rollout}%")
            elif a.resign:
                andysu("sign", "--channel", channel)
                print(f"{channel}: re-signed")
            continue
        seq = known_sequence(a.site, release)
        if top and seq < top["sequence"]:
            andysu("rollback", "--channel", channel, "--to", release)
            print(f"{channel}: rolled back from {top['id']} to {release}")
            continue
        args = ["publish", "--channel", channel, "--release", release]
        if rollout < 100:
            args += ["--rollout", str(rollout)]
        andysu(*args)
        print(f"{channel}: {top['id'] if top else 'nothing'} -> {release}" + (f" at {rollout}%" if rollout < 100 else ""))


if __name__ == "__main__":
    main()
