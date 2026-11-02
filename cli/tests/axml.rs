//! AXML round trip on a real aapt2-compiled manifest (the test host APK's).

use andysu_cli::axml;

#[test]
fn real_manifest_roundtrip() {
    let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/testapp-manifest.axml")).unwrap();
    let d = axml::parse(&b).unwrap();
    let w = axml::write(&d);
    let d2 = axml::parse(&w).unwrap();
    assert_eq!(d, d2);
    let a = andysu_cli::apk::manifest_info(&b).unwrap();
    let c = andysu_cli::apk::manifest_info(&w).unwrap();
    assert_eq!(format!("{a:?}"), format!("{c:?}"));
    assert_eq!(a.package, "com.andysu.testapp");
    assert!(a.activities.iter().any(|x| x == "com.andysu.host.PhoenixActivity"));
    // Write it out for aapt2 cross-checking when available.
    std::fs::write(std::env::temp_dir().join("andysu-axml-roundtrip.bin"), &w).unwrap();
}
