//! The person's rule for work notes, applied before any model: what comes out of a meeting, or speaks of the
//! person's work, goes to the vault `work`, never to another one. A small model had filed meetings elsewhere.
//!
//! - A recording whose file name says `reunion` or `meeting` (a meeting captured by the browser, a call…) is a meeting.
//! - A note that holds one of the person's own work words (`work_words` in the janitor's settings: a product, a
//!   company, a team) is work. The list is the person's: none is built in.
//! - Only « À trier » is concerned: the note is sent to the inbox of `work`, which files it as it files any note.
//! - A health note (`#santé`) is never taken: the health rule comes first.

/// The vault work notes go to.
pub const WORK: &str = "work";

/// Words of a recording's file name that say it is a meeting.
const MEETING_WORDS: [&str; 3] = ["reunion", "réunion", "meeting"];

/// Whether the note is a meeting (its recording's name says so) or holds one of the person's work words.
pub fn applies(text: &str, work_words: &[String]) -> bool {
    let lower = text.to_lowercase();
    audio_name(&lower).is_some_and(|n| MEETING_WORDS.iter().any(|w| n.contains(w)))
        || work_words
            .iter()
            .any(|w| !w.is_empty() && lower.contains(w.as_str()))
}

/// The file name after `audio ` in a dictation's text (`audio 2026-11-06-10h18-reunion-chrome.m4a`).
fn audio_name(lower: &str) -> Option<&str> {
    let at = lower.find("audio ")? + "audio ".len();
    let rest = &lower[at..];
    let mut end = rest.find(".m4a").map_or(rest.len().min(120), |e| e + 4);
    // 120 bytes may fall inside an accented letter.
    while !rest.is_char_boundary(end) {
        end -= 1;
    }
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::applies;

    fn words() -> Vec<String> {
        vec!["acme".into(), "project x".into()]
    }

    #[test]
    fn a_long_name_with_accents_and_no_extension_is_read_without_a_panic() {
        // Byte 120 after « audio » falls inside « é ».
        let text = format!("audio {}é then the note", "a".repeat(119));
        assert!(!applies(&text, &[]));
        assert!(applies(&format!("{text} about acme"), &words()));
    }

    #[test]
    fn a_meeting_recording_is_work_whatever_it_says() {
        assert!(applies(
            "Dictation of 2026-11-06 at 10:18, audio 2026-11-06-10h18-reunion-chrome-part-3.m4a (part 3): we said",
            &[]
        ));
        assert!(applies(
            "Dictation, audio 2026-11-05-18h28-meeting-facetime.m4a : ok",
            &[]
        ));
    }

    #[test]
    fn the_persons_own_words_are_work() {
        assert!(applies("an idea for the ACME dashboard", &words()));
        assert!(applies("notes on Project X", &words()));
    }

    #[test]
    fn nothing_is_work_without_a_meeting_or_a_word_of_theirs() {
        assert!(!applies(
            "Dictation, audio 2026-11-08-21h00-dictee.m4a : buy some milk",
            &words()
        ));
        assert!(!applies("slept badly, five hours", &words()));
        assert!(!applies("an idea for the ACME dashboard", &[]));
    }
}
