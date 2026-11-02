//! The manifest parser alone (reached only after a valid signature, but fuzzed anyway).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = andysu_core::manifest::Manifest::parse(data) {
        // Anything accepted must survive a round trip and select without panicking.
        let b = serde_json::to_vec(&m).unwrap();
        let m2 = andysu_core::manifest::Manifest::parse(&b).unwrap();
        assert_eq!(m, m2);
        let dev = andysu_core::select::Device { host_api: 1, shell_version: 1, api_level: 30, abis: vec!["x86_64".into()] };
        let _ = andysu_core::select::select(&m, &dev, Some(1), &[], "id");
    }
});
