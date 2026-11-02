//! GC after commit: cached blobs of plain components that sit in a kept slot are dropped
//! (`find_local` serves them from the slot file); cached zips are kept (a slot holds only the
//! extracted tree, and the zip is the delta base of the next release).

mod common;
use common::*;

use std::fs;
use std::io::Write;
use std::path::Path;

use andysu_core::manifest::{Apply, Archive, Kind, PatchFormat, Release};
use andysu_core::session::Session;
use andysu_core::store::Store;

fn lib(variant: u8) -> Vec<u8> {
    let mut v: Vec<u8> = (0..60_000u32).map(|i| (i.wrapping_mul(2654435761) >> 9) as u8).collect();
    if variant != 0 {
        v[777] ^= variant;
    }
    v
}

/// A zip with stored (uncompressed) entries, so a small change in an entry gives a small delta.
fn bundle(variant: u8) -> Vec<u8> {
    let mut z = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut z);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for i in 0..8u32 {
            w.start_file(format!("tracks/t{i}.dat"), opts).unwrap();
            let mut d: Vec<u8> =
                (0..5000u32).map(|j| (j.wrapping_mul(2246822519).wrapping_add(i) >> 11) as u8).collect();
            if i == 3 && variant != 0 {
                d[100] ^= variant;
            }
            w.write_all(&d).unwrap();
        }
        w.finish().unwrap();
    }
    z.into_inner()
}

fn release(remote: &Remote, id: &str, seq: u64, variant: u8) -> Release {
    let files = vec![
        File { path: "lib/x86_64/libgame.so", kind: Kind::Native, apply: Apply::Restart, data: lib(variant) },
        File { path: "assets/data", kind: Kind::Content, apply: Apply::Restart, data: bundle(variant) },
    ];
    let mut r = remote.release(id, seq, &files);
    r.components[2].archive = Some(Archive::Zip);
    r
}

fn release_b(remote: &Remote) -> Release {
    let mut r = release(remote, "2.0", 2, 1);
    remote.add_delta(&mut r, "lib/x86_64/libgame.so", &lib(0), &lib(1), PatchFormat::Zstd);
    remote.add_delta(&mut r, "assets/data", &bundle(0), &bundle(1), PatchFormat::Zstd);
    r
}

struct Installed {
    remote: Remote,
    dev: tempfile::TempDir,
    store: Store,
    a: Release,
}

/// Release A installed, launched, committed and garbage collected.
fn installed_a() -> Installed {
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let a = release(&remote, "1.0", 1, 0);
    let (b, s) = remote.publish(&Remote::manifest(1, vec![a.clone()]));
    let mut sess = session(dev.path());
    sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
    download_all(&mut sess);
    sess.stage().unwrap();
    let store = Store::open(dev.path()).unwrap();
    assert_eq!(store.launch([0; 16], NOW, true, 2).unwrap().slot.unwrap().id, "1.0");
    store.update_state(|s| s.mark_healthy()).unwrap();
    Installed { remote, dev, store, a }
}

fn gc(i: &Installed) -> andysu_core::store::GcReport {
    let st = i.store.load_state([0; 16]).unwrap();
    i.store.gc(&st, false, &[]).unwrap()
}

fn make_writable(p: &Path) {
    andysu_core::fsx::make_writable_recursive(p);
}

/// Check, fetch and stage release B, like a host would: a failed delta is retried via plan().
/// Returns the bytes "downloaded" and the kinds of the first plan.
fn update_to_b(i: &Installed, b: &Release) -> (u64, Vec<String>) {
    let (mb, sb) = i.remote.publish(&Remote::manifest(2, vec![b.clone()]));
    let mut sess: Session = session(i.dev.path());
    sess.check(&i.remote.manifest_url(), &mb, &sb, None, NOW).unwrap();
    let kinds: Vec<String> = sess.plan().unwrap().iter().map(|f| f.kind.clone()).collect();
    let mut bytes = 0;
    for _ in 0..4 {
        let plan = sess.plan().unwrap();
        if plan.is_empty() {
            break;
        }
        for f in plan {
            bytes += download(&f, None);
            let _ = sess.complete(&f.key); // a bad delta is blacklisted; the next plan() has the full blob
        }
    }
    sess.stage().unwrap();
    (bytes, kinds)
}

fn assert_b_staged(i: &Installed) {
    let slot = i.store.slot_dir("2.0");
    assert_eq!(fs::read(slot.join("lib/x86_64/libgame.so")).unwrap(), lib(1));
    let want = bundle(1);
    let zf = std::io::Cursor::new(want);
    let mut z = zip::ZipArchive::new(zf).unwrap();
    let mut t3 = vec![];
    std::io::Read::read_to_end(&mut z.by_name("tracks/t3.dat").unwrap(), &mut t3).unwrap();
    assert_eq!(fs::read(slot.join("assets/data/tracks/t3.dat")).unwrap(), t3);
}

#[test]
fn gc_drops_plain_blobs_held_by_slot_and_keeps_zip() {
    let i = installed_a();
    let lib_sha = &i.a.components[1].sha256;
    let desc_sha = &i.a.components[0].sha256;
    let zip_sha = &i.a.components[2].sha256;
    assert!(i.store.has_blob(lib_sha) && i.store.has_blob(zip_sha) && i.store.has_blob(desc_sha));

    let rep = gc(&i);
    assert!(rep.removed_slots.is_empty());
    assert!(!i.store.has_blob(lib_sha), "plain blob dropped");
    assert!(!i.store.has_blob(desc_sha), "descriptor follows the same rule");
    assert!(i.store.has_blob(zip_sha), "zip blob stays");
    assert_eq!(rep.removed_blobs, 2);
    assert_eq!(rep.removed_blobs_held_by_slots, 2);

    // The slot still launches and the bytes are findable.
    let slots = vec!["1.0".to_string()];
    let p = i.store.find_local(lib_sha, &slots).expect("served from the slot file");
    assert_eq!(p, i.store.slot_dir("1.0").join("lib/x86_64/libgame.so"));
    assert_eq!(fs::read(p).unwrap(), lib(0));
    assert_eq!(i.store.launch([0; 16], NOW, true, 2).unwrap().slot.unwrap().id, "1.0");
    assert_eq!(fs::read(i.store.slot_dir("1.0").join("assets/data/tracks/t3.dat")).unwrap().len(), 5000);

    // Idempotent; the zip is never dropped.
    let rep = gc(&i);
    assert_eq!((rep.removed_blobs, rep.removed_blobs_held_by_slots), (0, 0));
    assert!(i.store.has_blob(zip_sha));
}

#[test]
fn update_after_gc_uses_slot_file_and_kept_zip_as_delta_bases() {
    let i = installed_a();
    gc(&i);
    let b = release_b(&i.remote);
    let (bytes, kinds) = update_to_b(&i, &b);
    assert_eq!(kinds.iter().filter(|k| *k == "delta").count(), 2, "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "full").count(), 1, "only the descriptor: {kinds:?}");
    assert!(bytes < 3_000, "downloaded {bytes} bytes, full lib+zip would be {}", lib(1).len() + bundle(1).len());
    assert_b_staged(&i);
}

#[test]
fn damaged_slot_file_falls_back_to_full_download() {
    for damage in ["delete", "truncate", "corrupt"] {
        let i = installed_a();
        gc(&i);
        let f = i.store.slot_dir("1.0").join("lib/x86_64/libgame.so");
        make_writable(&f);
        match damage {
            "delete" => fs::remove_file(&f).unwrap(),
            "truncate" => fs::OpenOptions::new().write(true).open(&f).unwrap().set_len(1234).unwrap(),
            _ => {
                let mut d = fs::read(&f).unwrap();
                d[40_000] ^= 0xff;
                fs::write(&f, d).unwrap();
            }
        }
        let b = release_b(&i.remote);
        let (bytes, _) = update_to_b(&i, &b);
        assert!(bytes >= lib(1).len() as u64, "{damage}: the library was downloaded in full ({bytes})");
        assert_b_staged(&i);
    }
}

#[test]
fn gc_keeps_blob_when_slot_file_is_missing_or_short() {
    for damage in ["delete", "truncate"] {
        let i = installed_a();
        let f = i.store.slot_dir("1.0").join("lib/x86_64/libgame.so");
        // The slot file may share its inode with the blob: replace it by a private copy first.
        let data = fs::read(&f).unwrap();
        make_writable(&f);
        fs::remove_file(&f).unwrap();
        if damage == "truncate" {
            fs::write(&f, &data[..100]).unwrap();
        }
        let rep = gc(&i);
        assert!(i.store.has_blob(&i.a.components[1].sha256), "{damage}: no good copy in a slot, keep the blob");
        assert_eq!(rep.removed_blobs, 1, "{damage}: only the descriptor");
    }
}

#[test]
fn gc_keeps_blob_shared_with_an_archive_component() {
    // The same bytes as a plain file and as a zip: the zip reference wins.
    let remote = Remote::new();
    let dev = tempfile::tempdir().unwrap();
    let z = bundle(0);
    let files = vec![
        File { path: "lib/x86_64/libgame.so", kind: Kind::Native, apply: Apply::Restart, data: lib(0) },
        File { path: "assets/copy.zip", kind: Kind::Content, apply: Apply::Restart, data: z.clone() },
        File { path: "assets/data", kind: Kind::Content, apply: Apply::Restart, data: z.clone() },
    ];
    let mut r = remote.release("1.0", 1, &files);
    r.components[3].archive = Some(Archive::Zip);
    let (b, s) = remote.publish(&Remote::manifest(1, vec![r.clone()]));
    let mut sess = session(dev.path());
    sess.check(&remote.manifest_url(), &b, &s, None, NOW).unwrap();
    download_all(&mut sess);
    sess.stage().unwrap();
    let store = Store::open(dev.path()).unwrap();
    store.launch([0; 16], NOW, true, 2).unwrap();
    store.update_state(|s| s.mark_healthy()).unwrap();
    let st = store.load_state([0; 16]).unwrap();
    store.gc(&st, false, &[]).unwrap();
    assert!(store.has_blob(&r.components[3].sha256), "zip bytes kept although also a plain component");
    assert!(!store.has_blob(&r.components[1].sha256));
}
