#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Levels said on a named scale, tied to the sign of their phrase; measures said next to their word; and the properties
//! the detection keeps for any text. Ported with the same cases and results as the reference tests of the previous health
//! app (the reminder part of its note reader is not health and is not ported). Fictional notes only.

use memcp_health::{Lang, Measure, builtin, detect};

/// The numbers on ten the detection gives, by sign.
fn levels(text: &str) -> Vec<(String, String)> {
    detect(text, Lang::Fr)
        .signs
        .into_iter()
        .filter_map(|s| s.level.filter(|l| l.ends_with("/10")).map(|l| (s.id, l)))
        .collect()
}

fn lv(id: &str, n: u8) -> (String, String) {
    (id.to_string(), format!("{n}/10"))
}

#[test]
fn a_level_on_ten_goes_to_the_sign_of_its_phrase() {
    assert_eq!(levels("Anxiété à 2/10 ce matin"), vec![lv("anxiety", 2)]);
    assert_eq!(levels("je suis anxieux, 2 sur 10"), vec![lv("anxiety", 2)]);
    assert_eq!(
        levels("Angoisse 3 / 10. Mal au dos à 6/10"),
        vec![lv("anxiety", 3), lv("lower_back_pain", 6)]
    );
}

#[test]
fn a_number_without_its_scale_is_not_a_level() {
    assert!(levels("anxieux depuis 2 jours").is_empty());
    assert!(levels("anxiété à 12/10").is_empty());
    assert!(
        levels("triste et anxieux à 4/10").is_empty(),
        "two signs, one level: whose?"
    );
}

/// The mood, said from -5 to +5, is left to its bar; a sign noted by its presence has no level.
#[test]
fn the_mood_and_a_presence_sign_take_no_level() {
    assert!(levels("humeur à 3/10").is_empty());
    assert!(levels("fièvre 4/10").is_empty());
}

#[test]
fn a_level_said_in_words_and_two_levels_in_one_phrase() {
    let d = detect("Forte douleur thoracique ce soir", Lang::Fr);
    assert_eq!(d.signs[0].id, "chest_pain");
    assert_eq!(d.signs[0].level.as_deref(), Some("forte"));
    let d = detect("douleur 3/10 et fatigue 6/10", Lang::Fr);
    assert_eq!(d.signs[0].level.as_deref(), Some("3/10"));
    assert_eq!(d.signs[1].level.as_deref(), Some("6/10"));
    assert_eq!(d.signs[0].scale.as_deref(), Some("pain"));
    assert_eq!(d.signs[1].scale.as_deref(), Some("intensity"));
}

fn one(text: &str) -> Vec<(String, f64, Option<f64>)> {
    builtin()
        .measures(text, Lang::Fr)
        .into_iter()
        .map(|m| (m.id, m.value, m.second))
        .collect()
}

fn id(s: &str) -> String {
    s.to_string()
}

#[test]
fn a_pressure_said_in_centimetres_becomes_millimetres() {
    assert_eq!(
        one("J'ai une tension à 12/8 en sortie du médecin"),
        vec![(id("blood_pressure"), 120.0, Some(80.0))]
    );
    assert_eq!(
        one("tension 135 sur 85 ce matin"),
        vec![(id("blood_pressure"), 135.0, Some(85.0))]
    );
    assert_eq!(
        one("TA 14-9 au cabinet"),
        vec![(id("blood_pressure"), 140.0, Some(90.0))]
    );
}

#[test]
fn a_date_alone_is_never_a_pressure() {
    assert!(
        one("rendez-vous le 12/8 chez le dentiste")
            .iter()
            .all(|p| p.0 != "blood_pressure")
    );
}

#[test]
fn weight_sleep_pulse_glucose_temperature() {
    assert_eq!(one("je pèse 72,5 kg"), vec![(id("weight"), 72.5, None)]);
    assert_eq!(one("j'ai dormi 2 h"), vec![(id("sleep"), 2.0, None)]);
    assert_eq!(one("dormi 6h30 cette nuit"), vec![(id("sleep"), 6.5, None)]);
    assert_eq!(
        one("pouls à 64 au réveil"),
        vec![(id("heart_rate"), 64.0, None)]
    );
    assert_eq!(
        one("glycémie 1,05 à jeun"),
        vec![(id("glucose"), 1.05, None)]
    );
    assert_eq!(
        one("fièvre à 38,5 ce soir"),
        vec![(id("temperature"), 38.5, None)]
    );
}

#[test]
fn impossible_values_are_left_out() {
    assert!(one("je pèse 7 kg de plus").is_empty());
    assert!(one("pouls à 400").is_empty());
    assert!(
        one("tension 8/12").is_empty(),
        "a diastolic above the systolic is a mistake"
    );
}

#[test]
fn an_ordinary_note_proposes_nothing() {
    assert!(one("Idée : un jardin partagé dans la cour").is_empty());
    assert!(one("Réunion projet alpha, 3 actions").is_empty());
}

fn first(text: &str, lang: Lang) -> Option<Measure> {
    builtin().measures(text, lang).into_iter().next()
}

#[test]
fn units_are_kept_as_said_and_given_in_the_measures_own() {
    let m = first("tension 120/80 mmHg", Lang::Fr).expect("pressure");
    assert_eq!(
        (m.value, m.second, m.unit.as_str()),
        (120.0, Some(80.0), "mmHg")
    );
    assert_eq!(m.said_unit.as_deref(), Some("mmhg"));
    assert_eq!(m.raw, "120/80");
    let m = first("tension 12/8", Lang::Fr).expect("pressure");
    assert_eq!((m.said_unit, m.raw.as_str()), (None, "12/8"));
    let m = first("blood sugar 5.8 mmol/l", Lang::En).expect("glucose");
    assert_eq!((m.value, m.unit.as_str()), (5.8, "mmol/L"));
    let m = first("glycémie 110", Lang::Fr).expect("glucose");
    assert_eq!((m.value, m.unit.as_str()), (110.0, "mg/dL"));
    let m = first("I weigh 80.2 kg today", Lang::En).expect("weight");
    assert_eq!((m.value, m.unit.as_str()), (80.2, "kg"));
    let m = first("température 37.2 °C", Lang::Fr).expect("temperature");
    assert_eq!((m.value, m.unit.as_str()), (37.2, "°C"));
    // A number with a unit that says what it is needs no word.
    let m = first("64 bpm au repos", Lang::Fr).expect("heart rate");
    assert_eq!(m.id, "heart_rate");
    assert_eq!(
        first("saturation 97 %", Lang::Fr).map(|m| m.value),
        Some(97.0)
    );
}

#[test]
fn a_negated_sleep_is_not_a_duration() {
    assert!(one("j'ai pas dormi 3 heures").is_empty());
    assert!(one("never slept 3 hours").is_empty());
}

#[test]
fn json_form_of_a_detection() {
    let d = detect("Mal de tête 6/10, tension 12/8", Lang::Fr);
    let v = serde_json::to_value(&d).expect("json");
    assert_eq!(
        v["signs"][0],
        serde_json::json!({ "id": "headache", "label": "Mal de tête", "level": "6/10", "scale": "pain" })
    );
    assert_eq!(
        v["measures"][0],
        serde_json::json!({ "id": "blood_pressure", "value": 120.0, "second": 80.0, "unit": "mmHg" })
    );
    assert_eq!(v["red_flags"], serde_json::json!([]));
}

// ---- properties ----------------------------------------------------------------------------------------------------

/// A small deterministic generator (no dependency).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a>(&mut self, list: &[&'a str]) -> &'a str {
        list[(self.next() as usize) % list.len()]
    }
}

const PIECES: &[&str] = &[
    "tension",
    "poids",
    "pouls",
    "douleur",
    "mal au",
    "fièvre",
    "pas de",
    "sans",
    "12/8",
    "120/80",
    "72,5",
    "72.5",
    "kg",
    "mmHg",
    "°",
    "C",
    "2026-11-17",
    "17/11/2026",
    "12h30",
    "12:30",
    "6h",
    "dormi",
    "sur",
    "10",
    "/",
    "-",
    ":",
    ",",
    ".",
    "\n",
    "é",
    "œ",
    "ç",
    "😀",
    "\u{0301}",
    "٣",
    "０",
    "9999999999999999999999",
    "0",
    "à",
    "de",
    "j'ai",
    "1,",
    ",5",
    "h",
    "bpm",
    "g/l",
    "mg/dl",
    "spo2",
    " ",
    "  ",
    "\t",
    "'",
    "’",
    "idées noires",
    "en finir",
    "(",
    ")",
    "%",
    "°C",
    "°F",
];

#[test]
fn no_text_makes_the_detection_panic() {
    let mut rng = Rng(7);
    for _ in 0..3000 {
        let n = rng.next() % 14;
        let text: String = (0..n)
            .map(|_| rng.pick(PIECES))
            .collect::<Vec<_>>()
            .join(" ");
        for lang in [Lang::Auto, Lang::Fr, Lang::En, Lang::Es] {
            let d = detect(&text, lang);
            // Whatever the text, the values stay inside what a body can show.
            for m in &d.measures {
                assert!(
                    m.value.is_finite() && m.value >= 0.0 && m.value < 1000.0,
                    "{text:?}: {m:?}"
                );
            }
        }
    }
    // Long and odd inputs.
    for text in [
        String::new(),
        "\0".repeat(10),
        "9".repeat(5000),
        "é".repeat(5000),
        "douleur ".repeat(2000),
    ] {
        let _ = detect(&text, Lang::Auto);
    }
}

#[test]
fn dates_and_clock_times_never_become_values() {
    for text in [
        "tension du 2026-11-17",
        "poids le 17/11/2026",
        "rendez-vous à 12h30, tension à vérifier",
        "pouls mesuré à 12:30",
        "douleur depuis 2026-11-17",
        "le 12h30 douleur",
        "température le 2026-11-17 à 12h30",
        "glycémie à 8h",
        "fever on 2026-11-17 at 12:30",
        "poids 17/11/2026",
        "pouls 12:30",
        "tension 2026-11-17",
    ] {
        let d = detect(text, Lang::Auto);
        assert!(d.measures.is_empty(), "{text}: {:?}", d.measures);
        assert!(
            d.signs.iter().all(|s| s.level.is_none()),
            "{text}: {:?}",
            d.signs
        );
    }
    // A time right after a sign is no level, and a date or a time glued to random keywords never makes a value either.
    let mut rng = Rng(11);
    let stamps = [
        "2026-11-17",
        "17/11/2026",
        "12h30",
        "12:30",
        "8h",
        "23:59:59",
        "1/1/2000",
    ];
    let words = [
        "tension",
        "poids",
        "pouls",
        "glycémie",
        "température",
        "fièvre",
        "saturation",
        "douleur",
        "pese",
    ];
    for _ in 0..500 {
        let text = format!(
            "{} {} {}",
            rng.pick(&words),
            rng.pick(&["", "à", "de", "est", ":"]),
            rng.pick(&stamps)
        );
        let d = detect(&text, Lang::Auto);
        assert!(d.measures.is_empty(), "{text}: {:?}", d.measures);
        assert!(d.signs.iter().all(|s| s.level.is_none()), "{text}");
    }
}

/// A time of the `6h30` form is read only as a duration, after its keyword.
#[test]
fn a_clock_time_is_a_duration_only_after_dormi() {
    assert_eq!(one("dormi 6h30"), vec![(id("sleep"), 6.5, None)]);
    assert!(one("dormi à 12h30").is_empty());
    assert!(one("slept at 12:30").is_empty());
}

#[test]
fn a_comma_is_a_decimal_point() {
    for (text, id, v) in [
        ("poids 72,5 kg", "weight", 72.5),
        ("poids 72.5 kg", "weight", 72.5),
        ("glycémie 1,05", "glucose", 1.05),
        ("température 37,8", "temperature", 37.8),
        ("dormi 7,5 heures", "sleep", 7.5),
        ("fièvre à 38,5", "temperature", 38.5),
    ] {
        let m = first(text, Lang::Fr).unwrap_or_else(|| panic!("{text}"));
        assert_eq!((m.id.as_str(), m.value), (id, v), "{text}");
    }
    // A comma between two numbers of a list is not a decimal point: the pulse is 64, not 6.4.
    assert_eq!(
        one("pouls 64, 70 puis 72"),
        vec![(id("heart_rate"), 64.0, None)]
    );
    // A decimal pulse is no pulse.
    assert!(one("pouls 64,5").is_empty());
}

#[test]
fn every_measure_is_in_its_range_edges() {
    for (text, ok) in [
        ("poids 20 kg", true),
        ("poids 19 kg", false),
        ("poids 400 kg", true),
        ("poids 401 kg", false),
        ("pouls 20", true),
        ("pouls 19", false),
        ("pouls 250", true),
        ("pouls 251", false),
        ("température 45", true),
        ("température 46", false),
        ("saturation 100", true),
        ("saturation 49", false),
        ("tension 260/160", true),
        ("tension 261/160", false),
        ("dormi 16 h", true),
        ("dormi 17 h", false),
    ] {
        assert_eq!(!one(text).is_empty(), ok, "{text}");
    }
}

/// A level a later sign of the phrase closes belongs to the sign just before it; one that ends the phrase could be any.
#[test]
fn a_level_between_two_signs_goes_to_the_first() {
    assert_eq!(levels("Douleur à 4 sur 10 et fatigue"), vec![lv("pain", 4)]);
    assert!(levels("fatigue et douleur à 4 sur 10").is_empty());
}

#[test]
fn french_blood_pressure_accepts_dotted_pa_and_a_spoken_diastolic() {
    for (text, systolic, diastolic) in [
        ("P.A 172 86", 172.0, 86.0),
        ("P.A. 172 86", 172.0, 86.0),
        ("J’ai une tension de 17 neuf", 170.0, 90.0),
    ] {
        let found = detect(text, Lang::Fr).measures;
        assert_eq!(found.len(), 1, "{text}");
        assert_eq!(found[0].id, "blood_pressure");
        assert_eq!(found[0].value, systolic);
        assert_eq!(found[0].second, Some(diastolic));
        assert_eq!(found[0].unit, "mmHg");
    }
    assert!(
        detect("Article P.A. 17/11/2026", Lang::Fr)
            .measures
            .is_empty()
    );
}

#[test]
fn a_health_app_line_is_read_like_a_note() {
    // « Constantes du … : Tension 125/80 mmHg · Pouls 63 bpm · Saturation 96,8 % · Dormi 7,2 h · Sommeil 7 h 11 »
    let got = one("Tension 125/80 mmHg · Pouls 63 bpm · Saturation 96,8 % · Dormi 7,2 h");
    assert!(
        got.contains(&(id("blood_pressure"), 125.0, Some(80.0))),
        "{got:?}"
    );
    assert!(got.contains(&(id("heart_rate"), 63.0, None)), "{got:?}");
    assert!(
        got.contains(&(id("saturation"), 96.8, None)),
        "the % unit of the saturation: {got:?}"
    );
    assert!(got.contains(&(id("sleep"), 7.2, None)), "{got:?}");
    assert_eq!(one("sommeil 7,2 h"), vec![(id("sleep"), 7.2, None)]);
    let h11 = one("sommeil 7 h 11");
    assert_eq!(h11.len(), 1, "{h11:?}");
    assert!((h11[0].1 - (7.0 + 11.0 / 60.0)).abs() < 0.01, "{h11:?}");
}
