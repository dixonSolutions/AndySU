//! End-to-end flows through the sans-IO session against a file:// remote.

mod common;
use common::*;

use std::fs;

use andysu_core::manifest::{Apply, Kind, PatchFormat};
use andysu_core::session::CheckOutcome;
use andysu_core::{sig, Error};

fn files(v: &str) -> Vec<File<'static>> {
    vec![
        File {
            path: "lib/x86_64/libgame.so",
            kind: Kind::Native,
            apply: Apply::Restart,
            data: format!("ELF game {v}").repeat(500).into_bytes(),
        },
        File {
            path: "assets/data.txt",
            kind: Kind::Content,
            apply: Apply::Hot,
            data: format!("data {v}").into_bytes(),
        },
    ]
}

fn install(remote: &Remote, dev: &std::path::Path, m: &andysu_core::manifest::Manifest) -> CheckOutcome {
    let (b, s) = remote.publish(m);
    let mut sess = session(dev);
    let out = sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
    download_all(&mut sess);
    sess.stage().unwrap();
    out
}

#[test]
fn full_cycle_install_update_commit() {
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let r1 = remote.release("1.0", 1, &files("1"));
    let out = install(&remote, dev.path(), &Remote::manifest(1, vec![r1.clone()]));
    assert!(matches!(out, CheckOutcome::Update { ref release, hot_only: false, .. } if release == "1.0"), "{out:?}");

    let store = andysu_core::store::Store::open(dev.path()).unwrap();
    let d = store.launch([0; 16], NOW, true, 2).unwrap();
    assert_eq!(d.slot.as_ref().unwrap().id, "1.0");
    assert!(d.in_trial);
    let so = store.slot_dir("1.0").join("lib/x86_64/libgame.so");
    assert!(fs::metadata(&so).unwrap().permissions().readonly(), "code must be read-only");
    store.update_state(|s| s.mark_healthy()).unwrap();

    // Second release only changes hot content -> hot_only.
    let mut f2 = files("1");
    f2[1].data = b"data 2".to_vec();
    let r2 = remote.release("2.0", 2, &f2);
    let (b, s) = remote.publish(&Remote::manifest(2, vec![r1, r2]));
    let mut sess = session(dev.path());
    let out = sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
    match out {
        CheckOutcome::Update { release, hot_only, download_bytes, .. } => {
            assert_eq!(release, "2.0");
            assert!(hot_only);
            assert_eq!(
                download_bytes,
                6 + andysu_core::manifest::Manifest::parse(&b).unwrap().releases[1].components[0].size,
                "only changed blobs are downloaded"
            );
        }
        o => panic!("{o:?}"),
    }
    download_all(&mut sess);
    sess.stage().unwrap();
    let d = store.launch([0; 16], NOW, true, 2).unwrap();
    assert_eq!(d.slot.unwrap().id, "2.0");
    store.update_state(|s| s.mark_healthy()).unwrap();
    // Up to date now; a 304 path also works.
    let mut sess = session(dev.path());
    assert!(matches!(sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap(), CheckOutcome::UpToDate { .. }));
    assert!(matches!(sess.check_cached(NOW).unwrap(), CheckOutcome::UpToDate { .. }));
    // GC keeps current + previous.
    let st = store.load_state([0; 16]).unwrap();
    let rep = store.gc(&st, false, &[]).unwrap();
    assert!(rep.removed_slots.is_empty());
    let rep = store.gc(&st, true, &[]).unwrap();
    assert_eq!(rep.removed_slots, vec!["1.0".to_string()]);
    assert!(store.slot_dir("2.0").is_dir());
}

#[test]
fn rejects_tampering_downgrade_expiry_and_mismatch() {
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let r1 = remote.release("1.0", 1, &files("1"));
    let (b, s) = remote.publish(&Remote::manifest(5, vec![r1.clone()]));
    let url = remote.manifest_url();
    let mut sess = session(dev.path());

    // Tampered manifest bytes: rejected before parsing.
    let mut t = b.clone();
    let i = t.iter().position(|&c| c == b'1').unwrap();
    t[i] = b'2';
    assert_eq!(sess.check(&url, &t, &s, None, NOW), Err(Error::BadSignature));
    // Garbage that would not even parse: still a signature error, proving order of checks.
    assert_eq!(sess.check(&url, b"\xff\xfe not json", &s, None, NOW), Err(Error::BadSignature));
    // Signed by an unknown key.
    let rogue = andysu_core::keys::SecretKey::throwaway();
    let rs = sig::sign(&b, &rogue, None).unwrap();
    assert_eq!(sess.check(&url, &b, &rs, None, NOW), Err(Error::BadSignature));
    // Expired (freeze attack).
    assert!(matches!(sess.check(&url, &b, &s, None, NOW + 2 * 86400), Err(Error::Expired { .. })));
    // Wrong app / channel, correctly signed.
    let mut m = Remote::manifest(5, vec![r1.clone()]);
    m.app = "com.other".into();
    let (ob, os) = remote.publish(&m);
    assert!(matches!(sess.check(&url, &ob, &os, None, NOW), Err(Error::WrongApp { .. })));
    let mut m = Remote::manifest(5, vec![r1.clone()]);
    m.channel = "beta".into();
    let (ob, os) = remote.publish(&m);
    assert!(matches!(sess.check(&url, &ob, &os, None, NOW), Err(Error::WrongChannel { .. })));

    // Accept sequence 5, then a replayed sequence 4 is a rollback attack.
    assert!(sess.check(&url, &b, &s, None, NOW).is_ok());
    let (ob, os) = remote.publish(&Remote::manifest(4, vec![r1.clone()]));
    assert!(matches!(sess.check(&url, &ob, &os, None, NOW), Err(Error::SequenceRollback { last: 5, got: 4 })));
    // Same sequence, different content.
    let mut m = Remote::manifest(5, vec![r1.clone()]);
    m.releases[0].mandatory = true;
    let (ob, os) = remote.publish(&m);
    assert!(matches!(sess.check(&url, &ob, &os, None, NOW), Err(Error::SequenceReuse(5))));
    // The clock going backwards does not resurrect an expired manifest: now >= last issued.
    let mut m = Remote::manifest(6, vec![r1.clone()]);
    m.issued = andysu_core::time::format(NOW + 1000);
    m.expires = andysu_core::time::format(NOW + 2000);
    let (ob, os) = remote.publish(&m);
    assert!(sess.check(&url, &ob, &os, None, NOW).is_ok());
    let mut m = Remote::manifest(7, vec![r1.clone()]);
    m.expires = andysu_core::time::format(NOW + 500);
    let (ob, os) = remote.publish(&m);
    assert!(
        matches!(sess.check(&url, &ob, &os, None, NOW), Err(Error::Expired { .. })),
        "uses max(clock, last issued)"
    );

    // A blob replaced on the remote is caught by its hash.
    let remote2 = Remote::new();
    let dev2 = tempfile::tempdir().unwrap();
    let r = remote2.release("1.0", 1, &files("1"));
    let (b, s) = remote2.publish(&Remote::manifest(1, vec![r.clone()]));
    let target = &r.components[1];
    fs::write(remote2.dir.path().join("blobs/sha256").join(&target.sha256), vec![b'Z'; target.size as usize]).unwrap();
    let mut sess = session(dev2.path());
    sess.check(&remote2.manifest_url(), &b, &s, None, NOW).unwrap();
    let f = sess.plan().unwrap().into_iter().find(|f| f.key == target.sha256).unwrap();
    download(&f, None);
    assert!(matches!(sess.complete(&f.key), Err(Error::HashMismatch { .. })));
    assert!(!std::path::Path::new(&f.dest).exists(), "bad partial is deleted");
    assert!(sess.stage().is_err(), "cannot stage with a missing blob");
}

#[test]
fn certified_release_key() {
    use andysu_core::keys::SecretKey;
    use andysu_core::sig::Cert;
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let rel = SecretKey::throwaway();
    let cert = Cert::issue(&root_key(), &rel.public(), NOW - 10, NOW + 10);
    let m = Remote::manifest(1, vec![remote.release("1.0", 1, &files("1"))]);
    let b = serde_json::to_vec(&m).unwrap();
    let s = sig::sign(&b, &rel, Some(cert)).unwrap();
    let mut sess = session(dev.path());
    assert!(sess.check(&remote.manifest_url(), &b, &s, None, NOW).is_ok());
    let dev = tempfile::tempdir().unwrap();
    let mut sess = session(dev.path());
    assert!(matches!(sess.check(&remote.manifest_url(), &b, &s, None, NOW + 11), Err(Error::BadCert(_))));
}

#[test]
fn resume_interrupted_download() {
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let (b, s) = remote.publish(&Remote::manifest(1, vec![remote.release("1.0", 1, &files("1"))]));
    let mut sess = session(dev.path());
    sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
    let big = sess.plan().unwrap().into_iter().max_by_key(|f| f.size).unwrap();
    assert_eq!(big.offset, 0);
    assert_eq!(download(&big, Some(1000)), 1000); // connection drops
                                                  // New session (process restarted): the plan resumes from the partial.
    let mut sess = session(dev.path());
    assert!(matches!(sess.check_cached(NOW).unwrap(), CheckOutcome::Update { .. }));
    let again = sess.plan().unwrap().into_iter().find(|f| f.key == big.key).unwrap();
    assert_eq!(again.offset, 1000);
    download(&again, None);
    sess.complete(&again.key).unwrap();
    download_all(&mut sess);
    sess.stage().unwrap();
}

#[test]
fn deltas_and_fallback() {
    for fmt in [PatchFormat::Zstd, PatchFormat::Bsdiff] {
        let remote = Remote::new();
        let dev = tempfile::tempdir().unwrap();
        let f1 = files("1");
        let r1 = remote.release("1.0", 1, &f1);
        install(&remote, dev.path(), &Remote::manifest(1, vec![r1.clone()]));
        let store = andysu_core::store::Store::open(dev.path()).unwrap();
        store.launch([0; 16], NOW, true, 2).unwrap();
        store.update_state(|s| s.mark_healthy()).unwrap();

        let mut f2 = files("1");
        f2[0].data[100] ^= 0xff;
        let mut r2 = remote.release("2.0", 2, &f2);
        remote.add_delta(&mut r2, "lib/x86_64/libgame.so", &f1[0].data, &f2[0].data, fmt);
        let (b, s) = remote.publish(&Remote::manifest(2, vec![r2.clone()]));
        let mut sess = session(dev.path());
        sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
        let plan = sess.plan().unwrap();
        let d = plan.iter().find(|f| f.kind == "delta").expect("delta chosen");
        assert!(d.size < f2[0].data.len() as u64 / 10);
        download(d, None);
        sess.complete(&d.key).unwrap();
        download_all(&mut sess);
        sess.stage().unwrap();
        assert_eq!(fs::read(store.slot_dir("2.0").join("lib/x86_64/libgame.so")).unwrap(), f2[0].data);

        // A corrupt delta falls back to the full blob.
        let dev = tempfile::tempdir().unwrap();
        install(&remote, dev.path(), &Remote::manifest(3, vec![r1.clone()]));
        let store = andysu_core::store::Store::open(dev.path()).unwrap();
        store.launch([0; 16], NOW, true, 2).unwrap();
        store.update_state(|s| s.mark_healthy()).unwrap();
        let dpath = remote
            .dir
            .path()
            .join(format!("deltas/{}-{}", r2.components[1].deltas[0].from_sha256, r2.components[1].sha256))
            .join(&r2.components[1].deltas[0].sha256);
        let mut r2b = r2.clone();
        r2b.id = "2.1".into();
        r2b.sequence = 4;
        // A well-formed patch that produces the wrong bytes (as if built for another target).
        let mut other = f2[0].data.clone();
        other[200] ^= 0x55;
        let wrong = andysu_core::patch::create(fmt, &f1[0].data, &other).unwrap();
        fs::write(&dpath, &wrong).unwrap();
        let wh = andysu_core::hash::sha256_hex(&wrong);
        let nd = remote
            .dir
            .path()
            .join(format!("deltas/{}-{}", r2.components[1].deltas[0].from_sha256, r2.components[1].sha256));
        fs::write(nd.join(&wh), &wrong).unwrap();
        r2b.components[1].deltas[0].sha256 = wh;
        r2b.components[1].deltas[0].size = wrong.len() as u64;
        let (b, s) = remote.publish(&Remote::manifest(4, vec![r2b]));
        let mut sess = session(dev.path());
        sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
        let d = sess.plan().unwrap().into_iter().find(|f| f.kind == "delta").unwrap();
        download(&d, None);
        assert!(sess.complete(&d.key).is_err(), "{fmt:?}");
        let plan = sess.plan().unwrap();
        assert!(plan.iter().all(|f| f.kind == "full"), "falls back to full blob");
        download_all(&mut sess);
        sess.stage().unwrap();
    }
}

#[test]
fn rollback_flow_through_store() {
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let r1 = remote.release("1.0", 1, &files("1"));
    install(&remote, dev.path(), &Remote::manifest(1, vec![r1.clone()]));
    let store = andysu_core::store::Store::open(dev.path()).unwrap();
    store.launch([0; 16], NOW, true, 2).unwrap();
    store.update_state(|s| s.mark_healthy()).unwrap();
    let r2 = remote.release("2.0", 2, &files("2"));
    install(&remote, dev.path(), &Remote::manifest(2, vec![r1, r2]));
    // Launch into trial, payload throws during init -> immediate rollback.
    assert_eq!(store.launch([0; 16], NOW, true, 2).unwrap().slot.unwrap().id, "2.0");
    store.update_state(|s| s.report_failure("init exception")).unwrap();
    let d = store.launch([0; 16], NOW, true, 2).unwrap();
    assert_eq!(d.slot.unwrap().id, "1.0");
    // The bad release is not offered again.
    let mut sess = session(dev.path());
    assert!(matches!(sess.check_cached(NOW).unwrap(), CheckOutcome::UpToDate { .. }));
    // A slot deleted behind our back is treated as a failed boot.
    let r3 = remote.release("3.0", 3, &files("3"));
    install(&remote, dev.path(), &Remote::manifest(3, vec![r3]));
    andysu_core::fsx::remove_tree(&store.slot_dir("3.0")).unwrap();
    let d = store.launch([0; 16], NOW, true, 2).unwrap();
    assert_eq!(d.slot.unwrap().id, "1.0");
}

#[test]
fn zip_slip_component_is_refused_at_stage() {
    use std::io::Write;
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        w.start_file("../../escape.txt", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"pwned").unwrap();
        w.finish().unwrap();
    }
    let mut r = remote.release(
        "1.0",
        1,
        &[File { path: "assets", kind: Kind::Content, apply: Apply::Hot, data: buf.into_inner() }],
    );
    r.components[1].archive = Some(andysu_core::manifest::Archive::Zip);
    let (b, s) = remote.publish(&Remote::manifest(1, vec![r]));
    let mut sess = session(dev.path());
    sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
    download_all(&mut sess);
    assert!(matches!(sess.stage(), Err(Error::Archive(_))));
    assert!(!dev.path().join("escape.txt").exists());
    assert!(!dev.path().join("slots/escape.txt").exists());
    let st = andysu_core::state::load(&dev.path().join("state.json")).unwrap().unwrap();
    assert!(st.pending.is_none());
}
