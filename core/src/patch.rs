//! Binary deltas: `zstd --patch-from` frames and BSDIFF40 patches.

use std::io::{Read, Write};

use crate::manifest::PatchFormat;
use crate::{Error, Result};

/// zstd window must cover the reference plus the output.
fn window_log(total: usize) -> u32 {
    let mut w = 10u32;
    while (1usize << w) < total && w < 31 {
        w += 1;
    }
    w
}

pub fn create(format: PatchFormat, old: &[u8], new: &[u8]) -> Result<Vec<u8>> {
    match format {
        PatchFormat::Zstd => {
            let mut out = Vec::new();
            let mut enc = zstd::stream::write::Encoder::with_ref_prefix(&mut out, 19, old)
                .map_err(|e| Error::Patch(e.to_string()))?;
            enc.set_parameter(zstd::zstd_safe::CParameter::WindowLog(window_log(old.len().max(new.len()) + 1)))
                .map_err(|e| Error::Patch(e.to_string()))?;
            enc.set_parameter(zstd::zstd_safe::CParameter::EnableLongDistanceMatching(true))
                .map_err(|e| Error::Patch(e.to_string()))?;
            enc.set_pledged_src_size(Some(new.len() as u64)).map_err(|e| Error::Patch(e.to_string()))?;
            enc.write_all(new).map_err(|e| Error::Patch(e.to_string()))?;
            enc.finish().map_err(|e| Error::Patch(e.to_string()))?;
            Ok(out)
        }
        PatchFormat::Bsdiff => {
            let mut out = Vec::new();
            qbsdiff::Bsdiff::new(old, new)
                .compare(std::io::Cursor::new(&mut out))
                .map_err(|e| Error::Patch(e.to_string()))?;
            Ok(out)
        }
    }
}

/// Apply a patch. `max_out` bounds the output (the target component's declared size).
pub fn apply(format: PatchFormat, old: &[u8], patch: &[u8], max_out: u64) -> Result<Vec<u8>> {
    let out = match format {
        PatchFormat::Zstd => {
            let mut dec =
                zstd::stream::read::Decoder::with_ref_prefix(patch, old).map_err(|e| Error::Patch(e.to_string()))?;
            dec.set_parameter(zstd::zstd_safe::DParameter::WindowLogMax(31))
                .map_err(|e| Error::Patch(e.to_string()))?;
            let mut out = Vec::new();
            dec.take(max_out + 1).read_to_end(&mut out).map_err(|e| Error::Patch(e.to_string()))?;
            out
        }
        PatchFormat::Bsdiff => {
            let p = qbsdiff::Bspatch::new(patch).map_err(|e| Error::Patch(e.to_string()))?;
            if p.hint_target_size() > max_out {
                return Err(Error::Patch(format!("patch target {} bytes exceeds {max_out}", p.hint_target_size())));
            }
            let mut out = Vec::with_capacity(p.hint_target_size() as usize);
            p.apply(old, std::io::Cursor::new(&mut out)).map_err(|e| Error::Patch(e.to_string()))?;
            out
        }
    };
    if out.len() as u64 > max_out {
        return Err(Error::Patch(format!("patch output exceeds {max_out} bytes")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn mutate(base: &[u8], edits: &[(usize, u8)]) -> Vec<u8> {
        let mut v = base.to_vec();
        for &(i, b) in edits {
            if v.is_empty() {
                v.push(b);
            } else {
                let i = i % v.len();
                v[i] = b;
                if b % 7 == 0 {
                    v.insert(i, b);
                }
            }
        }
        v
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn roundtrip(base in proptest::collection::vec(any::<u8>(), 0..20000), edits in proptest::collection::vec((any::<usize>(), any::<u8>()), 0..40)) {
            let new = mutate(&base, &edits);
            for f in [PatchFormat::Zstd, PatchFormat::Bsdiff] {
                let p = create(f, &base, &new).unwrap();
                prop_assert_eq!(&apply(f, &base, &p, new.len() as u64).unwrap(), &new);
            }
        }
    }

    #[test]
    fn delta_is_small_for_small_change() {
        let base: Vec<u8> = (0..400_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let new = mutate(&base, &[(1000, 1), (200_000, 2), (399_000, 3)]);
        for f in [PatchFormat::Zstd, PatchFormat::Bsdiff] {
            let p = create(f, &base, &new).unwrap();
            assert!(p.len() < 4000, "{f:?} patch {} bytes", p.len());
            assert_eq!(apply(f, &base, &p, new.len() as u64).unwrap(), new);
        }
    }

    #[test]
    fn output_bound_and_garbage() {
        let base = vec![1u8; 1000];
        let new = vec![2u8; 5000];
        for f in [PatchFormat::Zstd, PatchFormat::Bsdiff] {
            let p = create(f, &base, &new).unwrap();
            assert!(apply(f, &base, &p, 4999).is_err());
            assert!(apply(f, &base, b"garbage!garbage!garbage!garbage!", 5000).is_err());
        }
    }
}
