//! Splits the input into (manifest, envelope) and runs the exact path a client runs on fetched
//! bytes: signature check first, then the parser. Must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let split = data.first().map_or(0, |&b| b as usize) % data.len().max(1);
    let (m, s) = data.split_at(split);
    let root = andysu_core::keys::PublicKey([0x3d; 32]);
    let _ = andysu_core::sig::verify(m, s, &[root], 1_800_000_000);
    let _ = andysu_core::manifest::Manifest::parse(m);
    let _ = andysu_core::payload::Descriptor::parse(m);
});
