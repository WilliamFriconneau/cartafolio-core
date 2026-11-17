use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch (0 if the clock is before it). The unit of every
/// timestamp stored by the core.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// `N` random bytes from the OS. A generator failure is unrecoverable:
/// we never fall back to a predictable identifier or token.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    if getrandom::fill(&mut b).is_err() {
        std::process::abort();
    }
    b
}

const B32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// `n` random lowercase base32 characters.
pub fn random_base32(n: usize) -> String {
    let mut out = String::with_capacity(n);
    let mut got = 0;
    while got < n {
        for byte in random_bytes::<16>() {
            if got == n {
                break;
            }
            out.push(B32[(byte & 31) as usize] as char);
            got += 1;
        }
    }
    out
}

/// Public identifier of a fact: `f_` + 12 characters.
pub fn new_fact_id() -> String {
    format!("f_{}", random_base32(12))
}

/// UTC (year, month, day, hour, minute, second) of a Unix timestamp (Howard Hinnant's algorithm).
pub fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// `YYYY-MM-DDTHHMMSS` (UTC), used in file and upload names.
pub fn compact(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}{mi:02}{s:02}")
}

/// `YYYY-MM-DDTHH:MM:SS+00:00` (UTC).
pub fn iso(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00")
}

/// Constant-time comparison of two digests.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_well_formed_and_distinct() {
        let a = new_fact_id();
        let b = new_fact_id();
        assert_eq!(a.len(), 14);
        assert!(a.starts_with("f_"));
        assert_ne!(a, b);
    }

    #[test]
    fn timestamps() {
        assert_eq!(compact(0), "1970-01-01T000000");
        assert_eq!(compact(1_790_812_800), "2026-10-01T000000");
        assert_eq!(iso(1_793_320_461), "2026-10-30T00:34:21+00:00");
        assert_eq!(civil(951_782_400).0, 2000); // 29 February 2000
        assert_eq!(compact(951_782_400), "2000-02-29T000000");
    }

    #[test]
    fn constant_time_equality() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
