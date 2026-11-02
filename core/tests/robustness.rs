//! Stable-toolchain fuzzing: random and mutated inputs never panic the parsers or verifier.
//! (The libFuzzer targets in core/fuzz run the same entry points on nightly.)

use proptest::prelude::*;

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../spec/fixtures/manifest/valid/full.json")).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn random_bytes(m in proptest::collection::vec(any::<u8>(), 0..600), s in proptest::collection::vec(any::<u8>(), 0..300)) {
        let root = andysu_core::keys::SecretKey::throwaway().public();
        prop_assert!(andysu_core::sig::verify(&m, &s, &[root], 0).is_err());
        let _ = andysu_core::manifest::Manifest::parse(&m);
        let _ = andysu_core::payload::Descriptor::parse(&m);
    }

    #[test]
    fn mutated_manifest(edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8)) {
        let mut b = fixture();
        for (i, v) in edits {
            let i = i.index(b.len());
            b[i] = v;
        }
        if let Ok(m) = andysu_core::manifest::Manifest::parse(&b) {
            let again = andysu_core::manifest::Manifest::parse(&serde_json::to_vec(&m).unwrap()).unwrap();
            prop_assert_eq!(m, again);
        }
    }
}
