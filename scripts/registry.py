#!/usr/bin/env python3
"""Maintains AndySU's static package registries: plain files served from GitHub Pages.

The site directory accumulates every released version (the release workflow keeps it on the
`gh-pages` branch). Each command adds one release's packages and rewrites the indexes:

  registry.py npm   --site S --base-url U pkg.tgz...      npm packuments (also UPM)
  registry.py cargo --site S --base-url U crate.crate...  cargo sparse index
  registry.py pub   --site S --base-url U pkg.tar.gz...   Hosted Pub Repository v2
  registry.py index --site S --base-url U               landing page listing everything

Layout under the site:
  maven/                            written by Gradle (maven-publish to a file repository)
  npm/<name>                        packument (unscoped names: UPM, CLI wrapper)
  npm/@scope/<name>, npm/@scope%2f<name>   packument for a scoped name, under both spellings
                                    because static hosts differ in decoding %2F
  npm/-/<file>.tgz                  tarballs
  cargo/config.json, cargo/<index path>, cargo/crates/<name>/<name>-<version>.crate
  pub/api/packages/<name>, pub/packages/<name>/versions/<version>.tar.gz
"""
import argparse
import base64
import hashlib
import io
import json
import os
import re
import shutil
import sys
import tarfile
import time

try:
    import tomllib
except ImportError:  # Python < 3.11
    tomllib = None


def write_json(path, obj):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=1, sort_keys=True)
    os.replace(tmp, path)


def read_json(path, default):
    try:
        with open(path) as f:
            return json.load(f)
    except FileNotFoundError:
        return default


def now_iso():
    return time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime())


# ---------------------------------------------------------------- npm

def npm_add(site, base, tgzs):
    root = os.path.join(site, "npm")
    for tgz in tgzs:
        data = open(tgz, "rb").read()
        with tarfile.open(fileobj=io.BytesIO(data)) as t:
            member = next(m for m in t.getmembers() if m.name.endswith("package.json") and m.name.count("/") == 1)
            pkg = json.load(t.extractfile(member))
        name, version = pkg["name"], pkg["version"]
        fname = f"{name.replace('/', '-').lstrip('@')}-{version}.tgz"
        os.makedirs(os.path.join(root, "-"), exist_ok=True)
        shutil.copyfile(tgz, os.path.join(root, "-", fname))
        paths = [os.path.join(root, name)]
        if name.startswith("@"):
            paths.append(os.path.join(root, name.replace("/", "%2f")))
        doc = read_json(paths[0], {"name": name, "versions": {}, "time": {}, "dist-tags": {}})
        manifest = {k: v for k, v in pkg.items() if k not in ("scripts", "devDependencies")}
        manifest["_id"] = f"{name}@{version}"
        manifest["dist"] = {
            "tarball": f"{base}/npm/-/{fname}",
            "shasum": hashlib.sha1(data).hexdigest(),
            "integrity": "sha512-" + base64.b64encode(hashlib.sha512(data).digest()).decode(),
        }
        doc["versions"][version] = manifest
        doc["time"][version] = now_iso()
        doc["time"]["modified"] = now_iso()
        doc["dist-tags"]["latest"] = max(doc["versions"], key=version_key)
        for k in ("description", "license", "repository", "displayName", "unity"):
            if k in pkg:
                doc[k] = pkg[k]
        for p in paths:
            write_json(p, doc)
        print(f"npm: {name}@{version}")


def version_key(v):
    parts = re.split(r"[.+-]", v)
    return [int(p) if p.isdigit() else -1 for p in parts]


# ---------------------------------------------------------------- cargo

def cargo_index_path(name):
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[0:2]}/{n[2:4]}/{n}"


CRATES_IO = "https://github.com/rust-lang/crates.io-index"


def cargo_deps(manifest, registry_index):
    """Index `deps` entries from a normalized (packaged) Cargo.toml."""
    out = []

    def add(table, kind, target=None):
        for key, spec in (table or {}).items():
            if isinstance(spec, str):
                spec = {"version": spec}
            dep = {
                "name": spec.get("package", key),
                "req": spec.get("version", "*"),
                "features": spec.get("features", []),
                "optional": spec.get("optional", False),
                "default_features": spec.get("default-features", spec.get("default_features", True)),
                "target": target,
                "kind": kind,
            }
            if "package" in spec:
                dep["name"], dep["package"] = key, spec["package"]
            reg = spec.get("registry-index")
            if reg is None:
                dep["registry"] = CRATES_IO
            elif reg.rstrip("/") != registry_index.rstrip("/"):
                dep["registry"] = reg
            out.append(dep)

    for section, kind in (("dependencies", "normal"), ("dev-dependencies", "dev"), ("build-dependencies", "build")):
        add(manifest.get(section), kind)
    for target, tables in (manifest.get("target") or {}).items():
        for section, kind in (("dependencies", "normal"), ("dev-dependencies", "dev"), ("build-dependencies", "build")):
            add(tables.get(section), kind, target)
    # Dev-dependencies are not part of the index contract for consumers; cargo ignores them.
    return [d for d in out if d["kind"] != "dev"]


def cargo_add(site, base, crates):
    if tomllib is None:
        sys.exit("cargo: Python 3.11+ is required (tomllib)")
    root = os.path.join(site, "cargo")
    index_url = f"sparse+{base}/cargo/"
    write_json(os.path.join(root, "config.json"),
               {"dl": f"{base}/cargo/crates/{{crate}}/{{crate}}-{{version}}.crate", "api": None})
    for path in crates:
        data = open(path, "rb").read()
        with tarfile.open(fileobj=io.BytesIO(data)) as t:
            member = next(m for m in t.getmembers() if m.name.count("/") == 1 and m.name.endswith("/Cargo.toml"))
            manifest = tomllib.loads(t.extractfile(member).read().decode())
        pkg = manifest["package"]
        name, version = pkg["name"], pkg["version"]
        dest = os.path.join(root, "crates", name, f"{name}-{version}.crate")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copyfile(path, dest)
        features = manifest.get("features", {})
        feats, feats2 = {}, {}
        for k, v in features.items():
            (feats2 if any(x.startswith("dep:") or "?/" in x for x in v) else feats)[k] = v
        entry = {
            "name": name, "vers": version, "deps": cargo_deps(manifest, index_url),
            "cksum": hashlib.sha256(data).hexdigest(), "features": feats, "yanked": False,
            "links": pkg.get("links"), "v": 2 if feats2 else 1,
        }
        if feats2:
            entry["features2"] = feats2
        if "rust-version" in pkg:
            entry["rust_version"] = pkg["rust-version"]
        ipath = os.path.join(root, cargo_index_path(name))
        os.makedirs(os.path.dirname(ipath), exist_ok=True)
        lines = []
        if os.path.exists(ipath):
            lines = [json.loads(line) for line in open(ipath) if line.strip()]
        lines = [e for e in lines if e["vers"] != version] + [entry]
        with open(ipath, "w") as f:
            for e in lines:
                f.write(json.dumps(e, separators=(",", ":")) + "\n")
        print(f"cargo: {name} {version}")


# ---------------------------------------------------------------- pub

def pub_add(site, base, archives):
    import yaml  # PyYAML
    root = os.path.join(site, "pub")
    for path in archives:
        data = open(path, "rb").read()
        with tarfile.open(fileobj=io.BytesIO(data)) as t:
            member = next(m for m in t.getmembers() if m.name.lstrip("./") == "pubspec.yaml")
            text = t.extractfile(member).read().decode()
        pubspec = yaml.safe_load(text)
        name, version = pubspec["name"], str(pubspec["version"])
        dest = os.path.join(root, "packages", name, "versions", f"{version}.tar.gz")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copyfile(path, dest)
        doc_path = os.path.join(root, "api", "packages", name)
        doc = read_json(doc_path, {"name": name, "versions": []})
        entry = {"version": version, "archive_url": f"{base}/pub/packages/{name}/versions/{version}.tar.gz",
                 "archive_sha256": hashlib.sha256(data).hexdigest(), "pubspec": pubspec, "published": now_iso()}
        doc["versions"] = [v for v in doc["versions"] if v["version"] != version] + [entry]
        doc["latest"] = max(doc["versions"], key=lambda v: version_key(v["version"]))
        write_json(doc_path, doc)
        print(f"pub: {name} {version}")


# ---------------------------------------------------------------- index

def index(site, base):
    rows = []
    for dirpath, _, files in os.walk(os.path.join(site, "maven")):
        if "maven-metadata.xml" in files:
            meta = open(os.path.join(dirpath, "maven-metadata.xml")).read()
            g = re.search(r"<groupId>(.*?)</groupId>", meta)
            a = re.search(r"<artifactId>(.*?)</artifactId>", meta)
            latest = re.search(r"<(?:release|latest)>(.*?)</", meta)
            if g and a:
                rows.append(("maven", f"{g.group(1)}:{a.group(1)}", latest.group(1) if latest else ""))
    npm = os.path.join(site, "npm")
    for dirpath, _, files in os.walk(npm):
        if os.path.basename(dirpath) == "-":
            continue
        for f in files:
            if "%2f" in f:
                continue
            doc = read_json(os.path.join(dirpath, f), None)
            if isinstance(doc, dict) and "dist-tags" in doc:
                rows.append(("npm", doc["name"], doc["dist-tags"]["latest"]))
    for dirpath, _, files in os.walk(os.path.join(site, "cargo")):
        if "crates" in dirpath.split(os.sep):
            continue
        for f in files:
            if f == "config.json":
                continue
            last = [json.loads(line) for line in open(os.path.join(dirpath, f)) if line.strip()][-1]
            rows.append(("cargo", last["name"], last["vers"]))
    pub_api = os.path.join(site, "pub", "api", "packages")
    for f in sorted(os.listdir(pub_api)) if os.path.isdir(pub_api) else []:
        doc = read_json(os.path.join(pub_api, f), None)
        rows.append(("pub", doc["name"], doc["latest"]["version"]))
    rows.sort()
    trs = "\n".join(f"<tr><td>{k}</td><td><code>{n}</code></td><td>{v}</td></tr>" for k, n, v in rows)
    html = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>AndySU packages</title>
<style>body{{font:16px/1.5 system-ui,sans-serif;max-width:52rem;margin:2rem auto;padding:0 1rem}}
td,th{{padding:.2rem .8rem;text-align:left}}code{{font-size:.9em}}</style></head>
<body><h1>AndySU packages</h1>
<p>Static registries for <a href="https://github.com/dixonSolutions/AndySU">AndySU</a>.
How to use them from each package manager: <a href="https://github.com/dixonSolutions/AndySU/blob/main/docs/REGISTRIES.md">docs/REGISTRIES.md</a>.</p>
<ul><li>Maven: <code>{base}/maven</code></li><li>npm / UPM: <code>{base}/npm</code></li>
<li>Cargo: <code>sparse+{base}/cargo/</code></li><li>pub: <code>{base}/pub</code></li></ul>
<table><tr><th>registry</th><th>package</th><th>latest</th></tr>
{trs}
</table></body></html>
"""
    with open(os.path.join(site, "index.html"), "w") as f:
        f.write(html)
    # GitHub Pages runs Jekyll by default, which drops files and directories starting with "_"
    # or "." and would mangle the extension-less JSON documents.
    open(os.path.join(site, ".nojekyll"), "w").close()
    print(f"index: {len(rows)} packages")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["npm", "cargo", "pub", "index"])
    ap.add_argument("files", nargs="*")
    ap.add_argument("--site", required=True)
    ap.add_argument("--base-url", required=True)
    a = ap.parse_args()
    base = a.base_url.rstrip("/")
    os.makedirs(a.site, exist_ok=True)
    if a.cmd == "npm":
        npm_add(a.site, base, a.files)
    elif a.cmd == "cargo":
        cargo_add(a.site, base, a.files)
    elif a.cmd == "pub":
        pub_add(a.site, base, a.files)
    index(a.site, base)


if __name__ == "__main__":
    main()
