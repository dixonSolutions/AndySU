//! Strict RFC 3339 UTC timestamps: `YYYY-MM-DDTHH:MM:SSZ`, nothing else.

use crate::{Error, Result};

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Howard Hinnant's algorithm.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` into Unix seconds.
pub fn parse(s: &str) -> Result<i64> {
    let b = s.as_bytes();
    let bad = || Error::Manifest(format!("bad timestamp {s:?}, expected YYYY-MM-DDTHH:MM:SSZ"));
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z'
    {
        return Err(bad());
    }
    let num = |r: std::ops::Range<usize>| -> Result<i64> {
        let mut v = 0i64;
        for &c in &b[r] {
            if !c.is_ascii_digit() {
                return Err(bad());
            }
            v = v * 10 + (c - b'0') as i64;
        }
        Ok(v)
    };
    let (y, mo, d, h, mi, se) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    let mdays = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mo) || d < 1 || d > mdays[(mo - 1) as usize] || h > 23 || mi > 59 || se > 59 {
        return Err(bad());
    }
    Ok(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se)
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format(t: i64) -> String {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        assert_eq!(parse("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(parse("2000-02-29T12:34:56Z").unwrap(), 951827696);
        assert_eq!(parse("2038-01-19T03:14:08Z").unwrap(), 1 << 31);
        assert!(parse("2001-02-29T00:00:00Z").is_err());
        assert!(parse("2026-13-01T00:00:00Z").is_err());
        assert!(parse("2026-11-01T24:00:00Z").is_err());
        assert!(parse("2026-11-01T00:00:00+00:00").is_err());
        assert!(parse("2026-11-01t00:00:00Z").is_err());
        assert!(parse("2026-11-01T00:00:00.5Z").is_err());
    }

    proptest::proptest! {
        #[test]
        fn roundtrip(t in 0i64..253402300799) {
            proptest::prop_assert_eq!(parse(&format(t)).unwrap(), t);
        }
    }
}
