//! A date and a time of day from a Unix time and the offset of the place, without a clock library:
//! the offset comes from the caller (an app knows its time zone; this crate may not ask the system).

/// (year, month, day, hour, minute) at `offset_minutes` east of UTC.
pub fn civil(unix: i64, offset_minutes: i32) -> (i64, u32, u32, u32, u32) {
    let local = unix + i64::from(offset_minutes) * 60;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    let hour = u32::try_from(secs / 3600).unwrap_or(0);
    let minute = u32::try_from(secs % 3600 / 60).unwrap_or(0);
    (year, month, day, hour, minute)
}

/// The Unix time of a civil date and time in UTC (Howard Hinnant's algorithm, the other way).
pub fn unix(y: i64, m: i64, d: i64, hh: i64, mm: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400 + hh * 3600 + mm * 60
}

/// The Unix time of `2026-11-02T10:46:17+00:00` (read as UTC: the memory writes UTC). `None` if it is not one.
pub fn parse_iso(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let mut t = time.get(..8)?.split(':');
    let (hh, mm, ss): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    Some(unix(y, m, day, hh, mm) + ss)
}

#[cfg(test)]
mod tests {
    use super::civil;

    #[test]
    fn a_civil_time_goes_back_to_unix() {
        assert_eq!(super::unix(2026, 11, 2, 12, 34), 1_793_622_840);
        assert_eq!(
            super::parse_iso("2024-02-29T00:00:00Z"),
            Some(1_709_164_800)
        );
        assert_eq!(super::parse_iso("nope"), None);
    }

    #[test]
    fn known_instants() {
        assert_eq!(civil(0, 0), (1970, 1, 1, 0, 0));
        // 2026-11-02 12:34 UTC
        assert_eq!(civil(1_793_622_840, 0), (2026, 11, 2, 12, 34));
        // The same instant in Paris in winter (+60): an hour later; at 23:30 UTC the next day.
        assert_eq!(civil(1_793_622_840, 60), (2026, 11, 2, 13, 34));
        assert_eq!(civil(1_793_622_840 + 11 * 3600, 60), (2026, 11, 3, 0, 34));
        // Leap day, and the day before the epoch.
        assert_eq!(civil(1_709_164_800, 0), (2024, 2, 29, 0, 0));
        assert_eq!(civil(-1, 0), (1969, 12, 31, 23, 59));
        assert_eq!(civil(0, -90), (1969, 12, 31, 22, 30));
    }
}
