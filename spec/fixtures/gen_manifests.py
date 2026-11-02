#!/usr/bin/env python3
"""Regenerates spec/fixtures/manifest/{valid,invalid}/*.json from one base manifest.

Each invalid fixture breaks exactly one rule; its file name says which. Both the JSON Schema and
the Rust core must reject every invalid fixture and accept every valid one (core/tests/spec.rs).
"""
import copy, json, os

H = lambda c: c * 64
BASE = {
    "schema": 1,
    "app": "com.example.app",
    "channel": "stable",
    "issued": "2026-11-01T00:00:00Z",
    "expires": "2099-01-01T00:00:00Z",
    "sequence": 7,
    "releases": [{
        "id": "1.2.0",
        "sequence": 3,
        "requires": {"host_api_min": 1, "shell_min": 1, "api_min": 24, "abis": ["arm64-v8a", "x86_64"]},
        "rollout": {"percent": 100, "salt": "1.2.0"},
        "mandatory": False,
        "notes_url": "https://example.com/notes/1.2.0",
        "components": [
            {"name": "descriptor", "kind": "content", "apply": "restart", "path": "andysu-payload.json",
             "sha256": H("a"), "size": 210},
            {"name": "classes", "kind": "dex", "apply": "restart", "path": "dex/classes.dex",
             "sha256": H("b"), "size": 120000, "urls": ["../blobs/sha256/" + H("b")],
             "deltas": [{"from_sha256": H("c"), "format": "zstd", "sha256": H("d"), "size": 900,
                         "urls": ["../deltas/" + H("c") + "-" + H("b") + "/" + H("d")]}]},
            {"name": "libgame", "kind": "native", "apply": "restart", "abi": "arm64-v8a",
             "path": "lib/arm64-v8a/libgame.0badf00d.so", "sha256": H("e"), "size": 500000},
            {"name": "assets", "kind": "content", "apply": "hot", "path": "assets", "archive": "zip",
             "unpacked_size": 90000, "sha256": H("f"), "size": 40000,
             "urls": ["https://cdn.example.com/blobs/sha256/" + H("f")]},
        ],
        "shell": {"version_code": 2, "sha256": H("1"), "size": 3000000, "urls": ["https://example.com/app-2.apk"]},
    }],
}

def minimal():
    m = copy.deepcopy(BASE)
    r = m["releases"][0]
    for k in ["requires", "rollout", "mandatory", "notes_url", "shell"]:
        del r[k]
    r["components"] = [{"name": "descriptor", "kind": "content", "path": "andysu-payload.json",
                        "sha256": H("a"), "size": 1}]
    return m

valid = {
    "full": BASE,
    "minimal": minimal(),
    "no-releases": {**copy.deepcopy(BASE), "releases": []},
    "unknown-fields-ignored": {**copy.deepcopy(BASE), "x_future": {"a": 1}},
}

def mut(fn):
    m = copy.deepcopy(BASE); fn(m); return m
R = lambda m: m["releases"][0]
C = lambda m, i=1: R(m)["components"][i]

invalid = {
    "schema-2": mut(lambda m: m.update(schema=2)),
    "missing-expires": mut(lambda m: m.pop("expires")),
    "missing-sequence": mut(lambda m: m.pop("sequence")),
    "negative-sequence": mut(lambda m: m.update(sequence=-1)),
    "sequence-not-integer": mut(lambda m: m.update(sequence=1.5)),
    "bad-time-format": mut(lambda m: m.update(issued="2026-11-01 00:00:00")),
    "time-with-offset": mut(lambda m: m.update(expires="2099-01-01T00:00:00+01:00")),
    "bad-channel": mut(lambda m: m.update(channel="Stable!")),
    "bad-app": mut(lambda m: m.update(app="com..example")),
    "sha-uppercase": mut(lambda m: C(m).update(sha256=H("B"))),
    "sha-short": mut(lambda m: C(m).update(sha256="ab" * 31)),
    "kind-unknown": mut(lambda m: C(m).update(kind="script")),
    "apply-unknown": mut(lambda m: C(m).update(apply="soon")),
    "path-absolute": mut(lambda m: C(m).update(path="/etc/passwd")),
    "path-dotdot": mut(lambda m: C(m).update(path="dex/../../x")),
    "path-dot-segment": mut(lambda m: C(m).update(path="./dex/classes.dex")),
    "path-backslash": mut(lambda m: C(m).update(path="dex\\classes.dex")),
    "path-double-slash": mut(lambda m: C(m).update(path="dex//classes.dex")),
    "path-trailing-slash": mut(lambda m: C(m).update(path="dex/")),
    "path-empty": mut(lambda m: C(m).update(path="")),
    "size-negative": mut(lambda m: C(m).update(size=-5)),
    "delta-format-unknown": mut(lambda m: C(m)["deltas"][0].update(format="xdelta")),
    "rollout-over-100": mut(lambda m: R(m)["rollout"].update(percent=101)),
    "no-descriptor": mut(lambda m: R(m).update(components=R(m)["components"][1:])),
    "no-components": mut(lambda m: R(m).update(components=[])),
    "release-id-bad": mut(lambda m: R(m).update(id="../1.2")),
    "archive-unknown": mut(lambda m: C(m, 3).update(archive="rar")),
    "releases-not-array": mut(lambda m: m.update(releases={})),
}

here = os.path.dirname(os.path.abspath(__file__))
for kind, items in [("valid", valid), ("invalid", invalid)]:
    d = os.path.join(here, "manifest", kind)
    os.makedirs(d, exist_ok=True)
    for name, m in items.items():
        with open(os.path.join(d, name + ".json"), "w") as f:
            json.dump(m, f, indent=2)
            f.write("\n")
print(len(valid), "valid,", len(invalid), "invalid")
