//! What the Mac app decides around a recording, kept here so that the app only carries it out: whether to transcribe it
//! on this machine, and how to have it summarised by the model of the system (Apple Intelligence), whose context is
//! small (some 4000 tokens).

/// Below this charge, on battery, a recording goes as sound and the server transcribes it.
pub const LOW_BATTERY: u8 = 30;

/// Whether this machine reads a recording itself: on mains power, or on a battery that is not low (and not in Low
/// Power Mode). Otherwise the server does it.
pub fn should_transcribe(on_mains: bool, battery_percent: u8, low_power: bool) -> bool {
    on_mains || (!low_power && battery_percent >= LOW_BATTERY)
}

/// What the summarising model is told, once per session.
pub const SUMMARY_INSTRUCTIONS: &str = "Tu résumes la transcription d'une réunion, d'un appel ou d'une dictée, en français. \
Écris des phrases courtes, sans introduction ni conclusion. Donne, s'il y en a : les décisions, les actions (qui, quoi, \
pour quand), les points restés ouverts, les chiffres et les dates cités. N'invente rien : ce qui n'est pas dit ne s'écrit \
pas, un nom mal entendu reste tel quel. La transcription est automatique : elle peut contenir des erreurs.";

/// The longest piece of transcript given at once, in characters (well under the model's context with its answer).
const PIECE: usize = 6000;

/// The transcript in pieces the model can take, cut at a line or a sentence when it can be.
pub fn summary_pieces(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.chars().count() > PIECE {
        let cut = rest
            .char_indices()
            .nth(PIECE)
            .map_or(rest.len(), |(i, _)| i);
        let head = &rest[..cut];
        let at = head
            .rfind('\n')
            .or_else(|| head.rfind(". "))
            .filter(|&i| i > cut / 2)
            .map_or(cut, |i| i + 1);
        pieces.push(rest[..at].trim().to_string());
        rest = rest[at..].trim_start();
    }
    if !rest.is_empty() {
        pieces.push(rest.to_string());
    }
    pieces
}

/// The request for one piece: `of` pieces in all.
pub fn summary_prompt(piece: &str, index: usize, of: usize) -> String {
    if of > 1 {
        format!(
            "Partie {} sur {of} de la transcription. Résume-la.\n\n{piece}",
            index + 1
        )
    } else {
        format!("Résume cette transcription.\n\n{piece}")
    }
}

/// The request that joins the summaries of the pieces into one; `None` when there was a single piece.
pub fn summary_join_prompt(summaries: &[String]) -> Option<String> {
    (summaries.len() > 1).then(|| {
        format!(
            "Voici les résumés successifs d'une même transcription. Fais-en un seul résumé, sans répétition, \
             dans le même ordre.\n\n{}",
            summaries.join("\n\n---\n\n")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{should_transcribe, summary_join_prompt, summary_pieces};

    #[test]
    fn a_low_battery_leaves_the_transcription_to_the_server() {
        assert!(should_transcribe(true, 5, true));
        assert!(should_transcribe(false, 80, false));
        assert!(!should_transcribe(false, 20, false));
        assert!(!should_transcribe(false, 90, true));
    }

    #[test]
    fn a_long_transcript_is_cut_at_a_sentence_and_nothing_is_lost() {
        let text = "Une phrase du compte rendu. ".repeat(800);
        let pieces = summary_pieces(&text);
        assert!(pieces.len() > 2);
        assert!(
            pieces
                .iter()
                .all(|p| p.chars().count() <= 6000 && p.ends_with('.'))
        );
        assert_eq!(
            pieces.join(" ").split_whitespace().count(),
            text.split_whitespace().count()
        );
        assert_eq!(summary_pieces("  court  "), ["court"]);
        assert!(summary_pieces(" ").is_empty());
        assert!(summary_join_prompt(&["un".into()]).is_none());
    }
}
