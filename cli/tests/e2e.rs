//! Step 3 end to end: the real `andysu` binary publishes to a site served over HTTP by the
//! built-in server; a simulated client checks, downloads (interrupted, then resumed), verifies,
//! stages, launches, commits, takes a delta update, rolls back, and rejects tampered, replayed
//! and expired manifests.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use andysu_cli::serve;
use serde_json::Value;

const APP: &str = "com.example.e2e";

struct Env {
    t: tempfile::TempDir,
}

impl Env {
    fn p(&self, s: &str) -> PathBuf {
        self.t.path().join(s)
    }
    fn andysu(&self, args: &[&str]) -> (bool, String, String) {
        let o = Command::new(env!("CARGO_BIN_EXE_andysu")).args(args).current_dir(self.t.path()).output().unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        )
    }
    fn ok(&self, args: &[&str]) -> String {
        let (ok, out, err) = self.andysu(args);
        assert!(ok, "andysu {args:?} failed:\n{out}\n{err}");
        out
    }
    fn fails(&self, args: &[&str]) -> String {
        let (ok, out, err) = self.andysu(args);
        assert!(!ok, "andysu {args:?} should fail:\n{out}");
        err
    }
    fn client(&self, url: &str) -> (bool, String, String) {
        let pubk = self.p("keys/andysu-public.pem");
        self.andysu(&[
            "client",
            "--root",
            self.p("device").to_str().unwrap(),
            "update",
            "--url",
            url,
            "--pub",
            pubk.to_str().unwrap(),
            "--app",
            APP,
            "--abi",
            "x86_64",
        ])
    }
    fn dev(&self, args: &[&str]) -> Value {
        let mut a = vec!["client", "--root", self.p("device").to_str().unwrap().to_owned().leak()];
        a.extend_from_slice(args);
        serde_json::from_str(&self.ok(&a)).unwrap()
    }
}

fn cc(dir: &Path, args: &[&str]) {
    assert!(Command::new("cc").args(args).current_dir(dir).status().unwrap().success(), "cc {args:?}");
}

/// A payload: libdep + libgame (game returns version*10), a big data file, a hot asset.
fn make_payload(dir: &Path, version: u32, perms: &[&str]) {
    let lib = dir.join("lib/x86_64");
    fs::create_dir_all(&lib).unwrap();
    fs::create_dir_all(dir.join("assets")).unwrap();
    fs::write(lib.join("dep.c"), format!("int dep_value(void) {{ return {version}; }}\n")).unwrap();
    fs::write(lib.join("game.c"), "int dep_value(void);\nint andysu_payload_v1(void) { return dep_value() * 10; }\n")
        .unwrap();
    cc(&lib, &["-shared", "-fPIC", "-Wl,-z,max-page-size=16384", "-Wl,-soname,libdep.so", "-o", "libdep.so", "dep.c"]);
    cc(
        &lib,
        &[
            "-shared",
            "-fPIC",
            "-Wl,-z,max-page-size=16384",
            "-Wl,-soname,libgame.so",
            "-o",
            "libgame.so",
            "game.c",
            "-L.",
            "-ldep",
        ],
    );
    fs::remove_file(lib.join("dep.c")).unwrap();
    fs::remove_file(lib.join("game.c")).unwrap();
    // 200 KB of pseudo-random data that changes slightly between versions (delta-friendly).
    let mut big: Vec<u8> = (0..200_000u32).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
    big[1234] = version as u8;
    fs::write(dir.join("assets/level.bin"), big).unwrap();
    fs::write(dir.join("assets/motd.txt"), format!("hello from v{version}")).unwrap();
    let perms: Vec<String> = perms.iter().map(|p| format!("\"{p}\"")).collect();
    fs::write(
        dir.join("andysu-payload.json"),
        format!(r#"{{"schema":1,"app":"{APP}","version":"{version}.0","host_api_min":1,"entry":{{"native_lib":"libgame","symbol":"andysu_payload_v1"}},"needs":{{"permissions":[{}]}}}}"#, perms.join(",")),
    )
    .unwrap();
}

fn slot_of(v: &Value) -> String {
    v["slot"]["id"].as_str().unwrap_or("").to_string()
}

#[test]
fn publish_update_resume_commit_delta_rollback_and_attacks() {
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("no C compiler; skipping");
        return;
    }
    let e = Env { t: tempfile::tempdir().unwrap() };
    let site = e.p("site");
    let s = site.to_str().unwrap();
    fs::create_dir_all(e.p("keys")).unwrap();
    e.ok(&["keygen", "--dir", "keys"]);
    let key = e.p("keys/andysu-root.key");
    let key = key.to_str().unwrap();
    fs::write(
        e.p("host.json"),
        format!(r#"{{"app":"{APP}","host_api":1,"permissions":["android.permission.INTERNET"]}}"#),
    )
    .unwrap();

    // v1
    make_payload(&e.p("p1"), 1, &[]);
    let rep: Value = serde_json::from_str(&e.ok(&[
        "pack",
        "p1",
        "--site",
        s,
        "--host",
        "host.json",
        "--hot",
        "assets/motd",
        "--require-16k",
    ]))
    .unwrap();
    assert_eq!(rep["release"], "1.0");
    assert!(rep["renamed"]["lib/x86_64/libgame.so"].as_str().unwrap().starts_with("lib/x86_64/libgame."));
    e.ok(&["publish", "--site", s, "--release", "1.0", "--key", key]);

    let srv = serve::spawn(
        &site,
        "127.0.0.1:0",
        serve::Faults { drop_after: Some(50_000), drop_prefix: "blobs/".into(), fail: vec![] },
    )
    .unwrap();
    let url = format!("{}/stable/update.json", srv.url());

    // Every blob's first response drops after 50 KB; the client resumes with Range requests.
    let out = e.ok(&[
        "client",
        "--root",
        e.p("device").to_str().unwrap(),
        "update",
        "--url",
        &url,
        "--pub",
        "keys/andysu-public.pem",
        "--app",
        APP,
    ]);
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["staged"], "1.0", "{out}");
    assert!(!r["resumed"].as_array().unwrap().is_empty(), "{out}");
    assert!(srv.requests().iter().any(|l| l.contains("range=50000 -> 206")), "{:?}", srv.requests());
    srv.set_faults(serve::Faults::default());

    // Launch: TRIAL, then healthy -> COMMITTED. The native lib really is renamed and read-only.
    let d = e.dev(&["launch"]);
    assert_eq!(slot_of(&d), "1.0");
    assert_eq!(d["in_trial"], true);
    let slot_dir = PathBuf::from(d["slot_dir"].as_str().unwrap());
    let desc: Value = serde_json::from_slice(&fs::read(slot_dir.join("andysu-payload.json")).unwrap()).unwrap();
    let order: Vec<&str> =
        desc["native"]["x86_64"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(order, vec!["libdep", "libgame"], "dependencies load first");
    let game = slot_dir.join(desc["native"]["x86_64"][1]["path"].as_str().unwrap());
    assert!(fs::metadata(&game).unwrap().permissions().readonly());
    let info: Value = serde_json::from_str(&e.ok(&["elf", game.to_str().unwrap()])).unwrap();
    assert!(info["needed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().starts_with("libdep.") && n != "libdep.so"));
    assert_eq!(info["aligned_16k"], true);
    e.dev(&["healthy"]);
    assert!(e.dev(&["state"])["trial"].is_null());

    // Unchanged remote: conditional GET answers 304 and nothing is downloaded.
    let r: Value = serde_json::from_str(&e.client(&url).1).unwrap();
    assert_eq!(r["not_modified"], true);
    assert_eq!(r["outcome"]["outcome"], "up_to_date");
    assert!(srv.requests().iter().any(|l| l.ends_with("update.json -> 304")));

    // v2 with deltas.
    make_payload(&e.p("p2"), 2, &[]);
    let rep: Value =
        serde_json::from_str(&e.ok(&["pack", "p2", "--site", s, "--host", "host.json", "--hot", "assets/motd"]))
            .unwrap();
    assert!(!rep["deltas"].as_array().unwrap().is_empty(), "{rep}");
    e.ok(&["publish", "--site", s, "--release", "2.0", "--key", key]);
    e.ok(&["verify", &url, "--pub", "keys/andysu-public.pem", "--app", APP, "--channel", "stable", "--blobs"]);
    let before = srv.requests().len();
    let r: Value = serde_json::from_str(&e.client(&url).1).unwrap();
    assert_eq!(r["staged"], "2.0");
    let reqs = srv.requests()[before..].to_vec();
    assert!(reqs.iter().any(|l| l.starts_with("GET /deltas/")), "delta used: {reqs:?}");
    let full_level = reqs.iter().filter(|l| l.contains("/blobs/")).count();
    assert!(full_level <= 4, "only small blobs fetched in full: {reqs:?}");

    // v2 launches into TRIAL and fails init: rolled back to 1.0, 2.0 marked bad.
    assert_eq!(slot_of(&e.dev(&["launch"])), "2.0");
    let f = e.dev(&["fail", "--reason", "init exception"]);
    assert_eq!(f["slot"]["id"], "1.0");
    let st = e.dev(&["state"]);
    assert_eq!(st["bad"][0], "2.0");
    assert_eq!(slot_of(&e.dev(&["launch"])), "1.0");
    let r: Value = serde_json::from_str(&e.client(&url).1).unwrap();
    assert_eq!(r["outcome"]["outcome"], "up_to_date", "a bad release is not offered again");

    // Remote-side rollback publishes 1.0's content as a new, higher release.
    let out = e.ok(&["rollback", "--site", s, "--to", "1.0", "--key", key]);
    assert!(out.contains("1.0-r3"), "{out}");
    let r: Value = serde_json::from_str(&e.client(&url).1).unwrap();
    assert_eq!(r["staged"], "1.0-r3");
    assert_eq!(r["downloaded_bytes"], 0, "content already cached");
    e.dev(&["launch"]);
    e.dev(&["healthy"]);

    // Attacks. Replay: keep the current manifest, publish newer, then serve the old one.
    let m = site.join("stable/update.json");
    let old = (fs::read(&m).unwrap(), fs::read(site.join("stable/update.json.sig")).unwrap());
    e.ok(&["sign", "--site", s, "--key", key]);
    assert!(e.client(&url).0);
    fs::write(&m, &old.0).unwrap();
    fs::write(site.join("stable/update.json.sig"), &old.1).unwrap();
    let (ok, _, err) = e.client(&url);
    assert!(!ok && err.contains("sequence_rollback"), "{err}");
    // Tamper: flip one byte of a correctly signed manifest.
    e.ok(&["sign", "--site", s, "--key", key]);
    let good = fs::read(&m).unwrap();
    let mut b = good.clone();
    let i = b.len() / 2;
    b[i] ^= 1;
    fs::write(&m, &b).unwrap();
    let (ok, _, err) = e.client(&url);
    assert!(!ok && err.contains("bad_signature"), "{err}");
    fs::write(&m, &good).unwrap();
    // Expired (freeze attack): a validly signed manifest that has already expired.
    e.ok(&["sign", "--site", s, "--key", key, "--expires-days", "0"]);
    let (ok, _, err) = e.client(&url);
    assert!(!ok && err.contains("expired"), "{err}");
    // Wrong key.
    fs::create_dir_all(e.p("other")).unwrap();
    e.ok(&["keygen", "--dir", "other"]);
    e.ok(&["sign", "--site", s, "--key", e.p("other/andysu-root.key").to_str().unwrap()]);
    let (ok, _, err) = e.client(&url);
    assert!(!ok && err.contains("bad_signature"), "{err}");

    // Release keys: a root-certified short-lived key works; the root stays offline.
    e.ok(&["keygen", "--dir", "keys", "--release-from", key, "--days", "30"]);
    e.ok(&["sign", "--site", s, "--key", "keys/andysu-release.key", "--cert", "keys/andysu-release.cert.json"]);
    assert!(e.client(&url).0);

    // The packer refuses a payload that needs more than the frozen host declares.
    make_payload(&e.p("p3"), 3, &["android.permission.CAMERA"]);
    let err = e.fails(&["pack", "p3", "--site", s, "--host", "host.json"]);
    assert!(err.contains("would need an APK update"), "{err}");

    // GC keeps current + previous only.
    let g = e.dev(&["gc"]);
    assert!(g["removed_slots"].as_array().unwrap().iter().any(|x| x == "2.0"), "{g}");
}
