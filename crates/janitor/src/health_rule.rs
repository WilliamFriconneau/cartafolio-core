//! The person's rule for health notes (2026-11-07), applied before any model: a note of an inbox that carries
//! `#santé` or `#sante` goes to the vault `health`, filed under `journal/<date>` and `sante`, its words untouched.
//!
//! - The date is the one written in the note, if any. Otherwise it is the day of its hour, and a note written before
//!   noon and before the person woke up counts for the day before (a day starts when one wakes up).
//! - Where it comes from (Obsidian, Notes, a dictation) stays in the text.
//! - A note of the vault `work` is never taken.

use serde_json::Value;

/// The vault health notes go to.
pub const HEALTH: &str = "health";
/// The vault this rule never takes a note from.
pub const NEVER_FROM: &str = "work";
/// The tag every health note gets, beside its day.
pub const TAG: &str = "sante";
/// When the hour of waking is not known: a note written before this hour counts for the day before.
pub const WAKE_UNKNOWN_HOUR: u32 = 5;

/// Whether the note asks for the health rule: `#santé` or `#sante`, as a word of its own.
pub fn applies(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["#santé", "#sante"].iter().any(|h| {
        lower.match_indices(h).any(|(at, m)| {
            lower[at + m.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
        })
    })
}

/// The first date written in the note: `2026-11-07`, `07/11/2026`, or `le 07/11` (the year of `this_year`).
pub fn written_date(text: &str, this_year: i64) -> Option<String> {
    let b = text.as_bytes();
    let digits = |from: usize, n: usize| -> Option<u32> {
        let s = b.get(from..from + n)?;
        if s.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(s).ok()?.parse().ok()
        } else {
            None
        }
    };
    let boundary = |at: usize| at == 0 || !b[at - 1].is_ascii_digit();
    let valid = |y: i64, m: u32, d: u32| (1..=12).contains(&m) && (1..=31).contains(&d) && y > 1900;
    for i in 0..b.len() {
        if !boundary(i) {
            continue;
        }
        // 2026-11-07
        if let (Some(y), Some(m), Some(d)) = (digits(i, 4), digits(i + 5, 2), digits(i + 8, 2))
            && b.get(i + 4) == Some(&b'-')
            && b.get(i + 7) == Some(&b'-')
            && valid(i64::from(y), m, d)
        {
            return Some(format!("{y:04}-{m:02}-{d:02}"));
        }
        // 07/11/2026 or 7/11 (one or two digits each)
        for dl in [2, 1] {
            let Some(d) = digits(i, dl) else { continue };
            if b.get(i + dl) != Some(&b'/') {
                continue;
            }
            for ml in [2, 1] {
                let Some(m) = digits(i + dl + 1, ml) else {
                    continue;
                };
                let after = i + dl + 1 + ml;
                if b.get(after).is_some_and(u8::is_ascii_digit) {
                    continue;
                }
                let y = if b.get(after) == Some(&b'/') {
                    match digits(after + 1, 4) {
                        Some(y) if !b.get(after + 5).is_some_and(u8::is_ascii_digit) => {
                            i64::from(y)
                        }
                        _ => continue,
                    }
                } else if text[..i].to_lowercase().trim_end().ends_with(" le")
                    || text[..i].to_lowercase().trim_end().ends_with(" du")
                    || matches!(text[..i].to_lowercase().trim_end(), "le" | "du")
                {
                    // Without a year, only « le 5/11 » or « du 5/11 »: « tension 12/8 » is not a date.
                    this_year
                } else {
                    continue;
                };
                if valid(y, m, d) {
                    return Some(format!("{y:04}-{m:02}-{d:02}"));
                }
            }
        }
    }
    None
}

/// The person's day of a note written at local `(y, m, d, hour, minute)`: the day before when it was written before
/// noon and before waking up (`wake` in minutes since midnight, if known; else before [`WAKE_UNKNOWN_HOUR`]).
pub fn day_of(local: (i64, u32, u32, u32, u32), wake: Option<u32>) -> String {
    let (y, m, d, hour, minute) = local;
    let at = hour * 60 + minute;
    let before_waking = match wake {
        Some(w) => at < w,
        None => hour < WAKE_UNKNOWN_HOUR,
    };
    if hour < 12 && before_waking {
        day_before(y, m, d)
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

fn day_before(y: i64, m: u32, d: u32) -> String {
    if d > 1 {
        return format!("{y:04}-{m:02}-{:02}", d - 1);
    }
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let leap = (py % 4 == 0 && py % 100 != 0) || py % 400 == 0;
    let last = match pm {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    format!("{py:04}-{pm:02}-{last:02}")
}

/// Whether a note is the day an app writes in the health space, `[<app>] day <date>: {json}`, whatever the app is called
/// (the iPhone app still signs it with its former name).
pub fn is_day_note(text: &str) -> bool {
    text.starts_with('[')
        && text
            .split_once("] day ")
            .is_some_and(|(app, _)| !app.contains(']'))
}

/// The hour of waking written by HealthApp in its day (`[HealthApp] day <date>: {..., "night": {"end": "...T07:12..."}}`),
/// in minutes since midnight.
pub fn wake_from_day(text: &str) -> Option<u32> {
    let at = text.find('{')?;
    let day: Value = serde_json::from_str(&text[at..]).ok()?;
    let end = day["night"]["end"].as_str()?;
    let t = end.split('T').nth(1)?;
    let h: u32 = t.get(0..2)?.parse().ok()?;
    let m: u32 = t.get(3..5)?.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Where the note comes from, as the person names it: a dictation, Obsidian, Notes, or the writer.
pub fn origin(note: &Value) -> String {
    let tags: Vec<&str> = note["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let path = note["source"]["path"].as_str().unwrap_or("");
    let author = note["author"].as_str().unwrap_or("");
    if tags.iter().any(|t| t.contains("dictee"))
        || note["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("Dictée"))
    {
        "dictée".into()
    } else if path.ends_with(".md") || tags.iter().any(|t| t.contains("obsidian")) {
        "Obsidian".into()
    } else if tags.iter().any(|t| t.ends_with("/notes") || *t == "notes")
        || author.contains("notes")
    {
        "Notes".into()
    } else if author.is_empty() {
        "boîte de notes".into()
    } else {
        author.into()
    }
}

/// The note as it is, with where it comes from added at the end when the text does not already say it.
pub fn with_origin(text: &str, origin: &str) -> String {
    let lower = text.to_lowercase();
    let said = lower.contains(&origin.to_lowercase())
        || (origin == "dictée" && lower.starts_with("dictée"));
    if said {
        text.to_string()
    } else {
        format!("{text}\n\n(source : {origin})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_hashtag_is_a_word_of_its_own() {
        assert!(applies("mal au dos ce matin #santé"));
        assert!(applies("#Sante, migraine"));
        assert!(applies("#santé."));
        assert!(!applies("#santépublique"));
        assert!(!applies("la santé du chat"));
    }

    #[test]
    fn a_written_date_wins() {
        assert_eq!(
            written_date("Dictée du 2026-11-06 à 17h05 #santé", 2026).as_deref(),
            Some("2026-11-06")
        );
        assert_eq!(
            written_date("le 05/11/2026 migraine", 2026).as_deref(),
            Some("2026-11-05")
        );
        assert_eq!(
            written_date("le 5/11 migraine", 2026).as_deref(),
            Some("2026-11-05")
        );
        assert_eq!(written_date("3 séries de 10, 80 kg", 2026), None);
        assert_eq!(written_date("tension 12/8", 2026), None);
        assert_eq!(
            written_date("Le 7/11, tension 12/8", 2026).as_deref(),
            Some("2026-11-07")
        );
        assert_eq!(written_date("rien", 2026), None);
    }

    #[test]
    fn before_noon_and_before_waking_is_the_day_before() {
        // Woke at 07:10: a note at 06:30 is yesterday's, one at 08:00 is today's.
        assert_eq!(day_of((2026, 11, 7, 6, 30), Some(430)), "2026-11-06");
        assert_eq!(day_of((2026, 11, 7, 8, 0), Some(430)), "2026-11-07");
        // Woke at 13:00 (a night shift): noon still closes the night.
        assert_eq!(day_of((2026, 11, 7, 12, 30), Some(780)), "2026-11-07");
        // Waking not known: before 5 h it is the night before.
        assert_eq!(day_of((2026, 11, 7, 2, 0), None), "2026-11-06");
        assert_eq!(day_of((2026, 11, 7, 9, 0), None), "2026-11-07");
        assert_eq!(day_of((2026, 3, 1, 1, 0), None), "2026-02-28");
        assert_eq!(day_of((2028, 3, 1, 1, 0), None), "2028-02-29");
        assert_eq!(day_of((2027, 1, 1, 1, 0), None), "2026-12-31");
    }

    #[test]
    fn the_hour_of_waking_is_read_from_the_healthapp_day() {
        let day = r#"[HealthApp] day 2026-11-07: {"date":"2026-11-07","night":{"start":"2026-11-06T23:40:00+01:00","end":"2026-11-07T07:12:00+01:00"}}"#;
        assert_eq!(wake_from_day(day), Some(432));
        assert_eq!(wake_from_day("[HealthApp] day 2026-11-07: {}"), None);
        assert!(is_day_note(day));
        assert!(is_day_note("[Other app] day 2026-11-07: {}"));
        assert!(!is_day_note("Slept badly, [HealthApp] day 2026-11-07"));
        assert!(!is_day_note("[a] note [b] day 2026-11-07: {}"));
    }

    #[test]
    fn where_it_comes_from_stays_in_the_text() {
        let dictation = json!({"text": "Dictée du 2026-11-06 : mal dormi #santé", "tags": ["inbox", "inbox/dictee"]});
        assert_eq!(origin(&dictation), "dictée");
        assert_eq!(
            with_origin("Dictée du 2026-11-06 : mal dormi #santé", "dictée"),
            "Dictée du 2026-11-06 : mal dormi #santé"
        );
        let obsidian = json!({"text": "migraine #sante", "tags": ["inbox"], "source": {"path": "Journal/2026-11-07.md"}});
        assert_eq!(origin(&obsidian), "Obsidian");
        assert_eq!(
            with_origin("migraine #sante", "Obsidian"),
            "migraine #sante\n\n(source : Obsidian)"
        );
        let device = json!({"text": "x", "tags": ["inbox"], "author": "device-laptop"});
        assert_eq!(origin(&device), "device-laptop");
    }
}
