//! The built-in health patterns, French and English: signs and symptoms (with the level as said) and measures (with the
//! value as said). Small, deterministic, no model. See docs/AUTOTAG.md for the table.
//!
//! The text is cut into tokens first: words, numbers (a decimal with a comma or a dot, a pair `12/8`), and the things that
//! look like numbers but are not measures: a date (`2026-11-17`, `17/11/2026`) and a time (`12h30`, `12:30`). Those are
//! never read as a value. A value counts only next to its word (« tension 12/8 », « je pèse 72,5 kg », « 38,5 de fièvre »).

use std::collections::BTreeMap;

pub(super) struct Found {
    pub tags: Vec<String>,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Num {
        text: String,
        pair: Option<String>,
        int: bool,
    },
    Time,
    Date,
    Sym(char),
}

fn tokenize(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = crate::text::fold(text).chars().collect();
    let n = chars.len();
    let digit = |k: usize| k < n && chars[k].is_ascii_digit();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while digit(i) {
                i += 1;
            }
            let int_part: String = chars[start..i].iter().collect();
            // 12:30, 12:30:15
            if i < n && chars[i] == ':' && digit(i + 1) {
                i += 1;
                while digit(i) {
                    i += 1;
                }
                if i < n && chars[i] == ':' && digit(i + 1) {
                    i += 1;
                    while digit(i) {
                        i += 1;
                    }
                }
                out.push(Tok::Time);
                continue;
            }
            // 12h30, 12h
            if i < n
                && chars[i] == 'h'
                && (digit(i + 1) || i + 1 >= n || !chars[i + 1].is_alphabetic())
            {
                i += 1;
                while digit(i) {
                    i += 1;
                }
                out.push(Tok::Time);
                continue;
            }
            // 2026-11-17
            if i < n && chars[i] == '-' && digit(i + 1) {
                let mut j = i + 1;
                while digit(j) {
                    j += 1;
                }
                if j < n && chars[j] == '-' && digit(j + 1) {
                    j += 1;
                    while digit(j) {
                        j += 1;
                    }
                    i = j;
                    out.push(Tok::Date);
                    continue;
                }
            }
            // 72,5 or 72.5
            if i + 1 < n && (chars[i] == '.' || chars[i] == ',') && digit(i + 1) {
                let mut j = i + 1;
                while digit(j) {
                    j += 1;
                }
                let frac: String = chars[i + 1..j].iter().collect();
                i = j;
                out.push(Tok::Num {
                    text: format!("{int_part}.{frac}"),
                    pair: None,
                    int: false,
                });
                continue;
            }
            // 12/8, and 17/11/2026 which is a date
            if i < n && chars[i] == '/' && digit(i + 1) {
                let mut j = i + 1;
                while digit(j) {
                    j += 1;
                }
                let second: String = chars[i + 1..j].iter().collect();
                if j < n && chars[j] == '/' && digit(j + 1) {
                    j += 1;
                    while digit(j) {
                        j += 1;
                    }
                    i = j;
                    out.push(Tok::Date);
                    continue;
                }
                i = j;
                out.push(Tok::Num {
                    text: int_part,
                    pair: Some(second),
                    int: true,
                });
                continue;
            }
            out.push(Tok::Num {
                text: int_part,
                pair: None,
                int: true,
            });
            continue;
        }
        if c.is_alphabetic() {
            let s = i;
            while i < n && chars[i].is_alphabetic() {
                i += 1;
            }
            out.push(Tok::Word(chars[s..i].iter().collect()));
            continue;
        }
        if !c.is_whitespace() {
            out.push(Tok::Sym(c));
        }
        i += 1;
    }
    out
}

fn is_word(t: Option<&Tok>, w: &str) -> bool {
    matches!(t, Some(Tok::Word(x)) if x == w)
}

fn phrase_at(toks: &[Tok], i: usize, phrase: &[&str]) -> bool {
    phrase
        .iter()
        .enumerate()
        .all(|(k, w)| is_word(toks.get(i + k), w))
}

const FILLER: [&str; 22] = [
    "est", "de", "d", "du", "a", "au", "aux", "of", "is", "at", "was", "etait", "vaut", "autour",
    "environ", "about", "around", "ce", "level", "niveau", "avec", "with",
];

/// The index of the first number after `from`, past a few filler words (« est », « de », « of », `:`).
fn next_value(toks: &[Tok], from: usize) -> Option<usize> {
    for (skipped, k) in (from..).enumerate() {
        match toks.get(k)? {
            Tok::Num { .. } => return Some(k),
            Tok::Word(w) if FILLER.contains(&w.as_str()) => {}
            Tok::Sym(':' | '=' | '\'' | '’') => {}
            _ => return None,
        }
        if skipped >= 5 {
            return None;
        }
    }
    None
}

fn number(t: &Tok) -> Option<f64> {
    match t {
        Tok::Num {
            text, pair: None, ..
        } => text.parse().ok(),
        _ => None,
    }
}

fn int_of(t: &Tok) -> Option<u32> {
    match t {
        Tok::Num {
            text,
            pair: None,
            int: true,
        } => text.parse().ok(),
        _ => None,
    }
}

fn text_of(t: &Tok) -> String {
    match t {
        Tok::Num { text, .. } => text.clone(),
        _ => String::new(),
    }
}

// ---- signs and symptoms ------------------------------------------------------------------------------------------------

type Phrases = &'static [&'static [&'static str]];

/// Sign slug and its phrases (folded: no accents). The specific ones come before the general ones: a phrase claims its words.
const SIGNS: [(&str, Phrases); 20] = [
    (
        "douleur-thoracique",
        &[&["douleur", "thoracique"], &["chest", "pain"]],
    ),
    (
        "douleur-abdominale",
        &[
            &["douleur", "abdominale"],
            &["mal", "au", "ventre"],
            &["abdominal", "pain"],
            &["stomach", "ache"],
            &["stomachache"],
        ],
    ),
    (
        "douleur-dos",
        &[
            &["mal", "de", "dos"],
            &["mal", "au", "dos"],
            &["back", "pain"],
        ],
    ),
    (
        "douleur-gorge",
        &[&["mal", "de", "gorge"], &["sore", "throat"]],
    ),
    (
        "cephalee",
        &[
            &["cephalee"],
            &["cephalees"],
            &["migraine"],
            &["migraines"],
            &["headache"],
            &["headaches"],
            &["mal", "de", "tete"],
            &["mal", "a", "la", "tete"],
        ],
    ),
    (
        "nausee",
        &[
            &["nausee"],
            &["nausees"],
            &["nausea"],
            &["nauseous"],
            &["mal", "au", "coeur"],
        ],
    ),
    (
        "vomissement",
        &[
            &["vomissement"],
            &["vomissements"],
            &["vomi"],
            &["vomis"],
            &["vomiting"],
            &["vomited"],
        ],
    ),
    (
        "vertige",
        &[
            &["vertige"],
            &["vertiges"],
            &["dizzy"],
            &["dizziness"],
            &["vertigo"],
        ],
    ),
    ("toux", &[&["toux"], &["cough"], &["coughing"]]),
    (
        "essoufflement",
        &[
            &["essoufflement"],
            &["essouffle"],
            &["essoufflee"],
            &["souffle", "court"],
            &["shortness", "of", "breath"],
            &["short", "of", "breath"],
            &["dyspnee"],
        ],
    ),
    ("palpitations", &[&["palpitation"], &["palpitations"]]),
    ("diarrhee", &[&["diarrhee"], &["diarrhea"], &["diarrhoea"]]),
    ("constipation", &[&["constipation"], &["constipated"]]),
    (
        "insomnie",
        &[&["insomnie"], &["insomnia"], &["mal", "dormi"]],
    ),
    (
        "anxiete",
        &[
            &["anxiete"],
            &["anxiety"],
            &["anxious"],
            &["anxieux"],
            &["angoisse"],
            &["angoisses"],
        ],
    ),
    ("frissons", &[&["frisson"], &["frissons"], &["chills"]]),
    (
        "demangeaison",
        &[
            &["demangeaison"],
            &["demangeaisons"],
            &["itching"],
            &["itchy"],
        ],
    ),
    ("eruption", &[&["eruption"], &["rash"]]),
    ("fievre", &[&["fievre"], &["fever"], &["febrile"]]),
    // The general ones last.
    (
        "fatigue",
        &[
            &["fatigue"],
            &["fatigues"],
            &["fatiguee"],
            &["tired"],
            &["exhausted"],
            &["epuise"],
            &["epuisee"],
            &["exhaustion"],
            &["asthenie"],
        ],
    ),
];

/// « douleur » and « pain » come after the specific pains, so that « douleur thoracique » is not also « douleur ».
const PAIN: Phrases = &[
    &["douleur"],
    &["douleurs"],
    &["pain"],
    &["pains"],
    &["ache"],
    &["aches"],
    &["mal", "au"],
    &["mal", "aux"],
    &["mal", "a"],
];

const QUALITY: [&str; 18] = [
    "leger", "legere", "legers", "legeres", "faible", "faibles", "modere", "moderee", "fort",
    "forte", "forts", "fortes", "intense", "intenses", "severe", "severes", "mild", "moderate",
];
const QUALITY_MORE: [&str; 3] = ["strong", "slight", "important"];

fn is_quality(t: Option<&Tok>) -> Option<&str> {
    match t {
        Some(Tok::Word(w))
            if QUALITY.contains(&w.as_str()) || QUALITY_MORE.contains(&w.as_str()) =>
        {
            Some(w.as_str())
        }
        _ => None,
    }
}

/// The level said next to a sign: `n/10` (« 2/10 », « 2 sur 10 », « 7 out of 10 »), or the word (« forte », « mild »).
fn level(toks: &[Tok], start: usize, end: usize) -> Option<String> {
    if let Some(k) = next_value(toks, end)
        && let Tok::Num {
            text,
            pair,
            int: true,
        } = &toks[k]
        && let Ok(n) = text.parse::<u32>()
        && n <= 10
    {
        match pair.as_deref() {
            Some("10") => return Some(format!("{n}/10")),
            Some(_) => {}
            None => {
                let mut j = k + 1;
                if is_word(toks.get(j), "sur") || is_slash(toks.get(j)) {
                    j += 1;
                } else if is_word(toks.get(j), "out") && is_word(toks.get(j + 1), "of") {
                    j += 2;
                } else {
                    j = 0;
                }
                if j > 0
                    && matches!(toks.get(j), Some(Tok::Num { text, pair: None, .. }) if text == "10")
                {
                    return Some(format!("{n}/10"));
                }
            }
        }
    }
    for k in end..end + 3 {
        if let Some(q) = is_quality(toks.get(k)) {
            return Some(q.to_string());
        }
        if matches!(toks.get(k), Some(Tok::Num { .. } | Tok::Date | Tok::Time)) {
            break;
        }
    }
    if start > 0
        && let Some(q) = is_quality(toks.get(start - 1))
    {
        return Some(q.to_string());
    }
    None
}

// ---- measures ----------------------------------------------------------------------------------------------------------

fn unit_word(toks: &[Tok], k: usize, table: &[(&str, &'static str)]) -> Option<&'static str> {
    // `k` is the number: the unit is the word after it (`mg/dl`: three tokens).
    let w = match toks.get(k + 1)? {
        Tok::Word(w) => w.as_str(),
        _ => return None,
    };
    if is_slash(toks.get(k + 2)) && matches!(toks.get(k + 3), Some(Tok::Word(_))) {
        let Some(Tok::Word(w2)) = toks.get(k + 3) else {
            return None;
        };
        let joined = format!("{w}/{w2}");
        return table.iter().find(|(s, _)| *s == joined).map(|(_, u)| *u);
    }
    table.iter().find(|(s, _)| *s == w).map(|(_, u)| *u)
}

fn is_slash(t: Option<&Tok>) -> bool {
    matches!(t, Some(Tok::Sym('/')))
}

type Value = (String, Option<&'static str>);

fn tension(toks: &[Tok], from: usize) -> Option<Value> {
    let k = next_value(toks, from)?;
    let (s, d, last) = match &toks[k] {
        Tok::Num {
            text,
            pair: Some(p),
            int: true,
        } => (text.parse::<u32>().ok()?, p.parse::<u32>().ok()?, k),
        t => {
            let s = int_of(t)?;
            let j = k + 1;
            if is_word(toks.get(j), "sur") || is_word(toks.get(j), "over") {
                (s, int_of(toks.get(j + 1)?)?, j + 1)
            } else {
                (s, int_of(toks.get(j)?)?, j)
            }
        }
    };
    // cmHg (12/8) or mmHg (120/80).
    let ok = ((5..=30).contains(&s) && (3..=20).contains(&d) && s > d)
        || ((60..=260).contains(&s) && (30..=160).contains(&d) && s > d);
    if !ok {
        return None;
    }
    let unit = match toks.get(last + 1) {
        Some(Tok::Word(w)) if w == "mmhg" => Some("mmhg"),
        Some(Tok::Word(w)) if w == "cmhg" => Some("cmhg"),
        _ => None,
    };
    Some((format!("{s}/{d}"), unit))
}

fn weight(toks: &[Tok], from: usize) -> Option<Value> {
    let k = next_value(toks, from)?;
    let v = number(&toks[k])?;
    let unit = unit_word(
        toks,
        k,
        &[
            ("kg", "kg"),
            ("kgs", "kg"),
            ("kilo", "kg"),
            ("kilos", "kg"),
            ("kilogramme", "kg"),
            ("kilogrammes", "kg"),
            ("lb", "lb"),
            ("lbs", "lb"),
            ("pound", "lb"),
            ("pounds", "lb"),
            ("livres", "lb"),
        ],
    );
    let ok = match unit {
        Some("lb") => (40.0..=900.0).contains(&v),
        _ => (20.0..=400.0).contains(&v),
    };
    ok.then(|| (text_of(&toks[k]), unit))
}

fn pulse(toks: &[Tok], from: usize) -> Option<Value> {
    let k = next_value(toks, from)?;
    let v = int_of(&toks[k])?;
    let unit = unit_word(toks, k, &[("bpm", "bpm"), ("battements", "bpm")]);
    (20..=250).contains(&v).then(|| (v.to_string(), unit))
}

fn glycemia(toks: &[Tok], from: usize) -> Option<Value> {
    let k = next_value(toks, from)?;
    let v = number(&toks[k])?;
    let unit = unit_word(
        toks,
        k,
        &[
            ("g/l", "gl"),
            ("gl", "gl"),
            ("mmol/l", "mmoll"),
            ("mmol", "mmoll"),
            ("mg/dl", "mgdl"),
            ("mgdl", "mgdl"),
        ],
    );
    let ok = match unit {
        Some("mgdl") => (20.0..=800.0).contains(&v),
        _ => (0.2..=40.0).contains(&v),
    };
    ok.then(|| (text_of(&toks[k]), unit))
}

fn temperature_value(toks: &[Tok], k: usize) -> Option<Value> {
    let v = number(toks.get(k)?)?;
    let unit = if matches!(toks.get(k + 1), Some(Tok::Sym('°'))) {
        match toks.get(k + 2) {
            Some(Tok::Word(w)) if w == "f" => Some("f"),
            Some(Tok::Word(w)) if w == "c" => Some("c"),
            _ => None,
        }
    } else {
        None
    };
    let ok = if unit == Some("f") {
        (90.0..=110.0).contains(&v)
    } else {
        (30.0..=45.0).contains(&v)
    };
    ok.then(|| (text_of(&toks[k]), unit))
}

fn temperature(toks: &[Tok], from: usize) -> Option<Value> {
    temperature_value(toks, next_value(toks, from)?)
}

fn saturation(toks: &[Tok], from: usize) -> Option<Value> {
    let k = next_value(toks, from)?;
    let v = number(&toks[k])?;
    (50.0..=100.0)
        .contains(&v)
        .then(|| (text_of(&toks[k]), None))
}

type Reader = fn(&[Tok], usize) -> Option<Value>;

const MEASURES: [(&str, Phrases, Reader); 6] = [
    (
        "tension",
        &[
            &["tension"],
            &["pression", "arterielle"],
            &["blood", "pressure"],
            &["bp"],
        ],
        tension,
    ),
    (
        "poids",
        &[
            &["poids"],
            &["pese"],
            &["weight"],
            &["weigh"],
            &["weighs"],
            &["weighed"],
        ],
        weight,
    ),
    (
        "pouls",
        &[
            &["pouls"],
            &["frequence", "cardiaque"],
            &["pulse"],
            &["heart", "rate"],
            &["fc"],
        ],
        pulse,
    ),
    (
        "glycemie",
        &[
            &["glycemie"],
            &["glucose"],
            &["blood", "sugar"],
            &["blood", "glucose"],
            &["glycaemia"],
        ],
        glycemia,
    ),
    (
        "temperature",
        &[&["temperature"], &["temp"], &["fievre"], &["fever"]],
        temperature,
    ),
    ("saturation", &[&["saturation"], &["spo2"]], saturation),
];

/// Finds the signs and the measures of a text.
pub(super) fn health(text: &str) -> Found {
    let toks = tokenize(text);
    let mut tags: Vec<String> = Vec::new();
    let mut fields = BTreeMap::new();
    let mut claimed = vec![false; toks.len()];
    // Signs, specific first, each phrase claiming its words.
    let mut levels: Vec<(String, String)> = Vec::new();
    let all_signs = SIGNS
        .iter()
        .map(|(s, p)| (*s, *p))
        .chain(std::iter::once(("douleur", PAIN)));
    for (slug, phrases) in all_signs {
        let mut found: Option<(usize, usize)> = None;
        for i in 0..toks.len() {
            if claimed[i] {
                continue;
            }
            // The longest phrase at this place.
            let hit = phrases
                .iter()
                .filter(|p| phrase_at(&toks, i, p) && !(i..i + p.len()).any(|k| claimed[k]))
                .map(|p| p.len())
                .max();
            if let Some(len) = hit {
                if found.is_none() {
                    found = Some((i, i + len));
                }
                for c in claimed.iter_mut().skip(i).take(len) {
                    *c = true;
                }
            }
        }
        if let Some((s, e)) = found {
            tags.push(format!("signe/{slug}"));
            if let Some(l) = level(&toks, s, e) {
                levels.push((slug.replace('-', "_"), l));
            }
        }
    }
    match levels.as_slice() {
        [] => {}
        [(_, l)] => {
            fields.insert("level".to_string(), l.clone());
        }
        many => {
            for (slug, l) in many {
                fields.insert(format!("level_{slug}"), l.clone());
            }
        }
    }
    // Measures: the first one said of each kind.
    for (name, keys, read) in MEASURES {
        let mut value: Option<Value> = None;
        'scan: for i in 0..toks.len() {
            for key in keys {
                if phrase_at(&toks, i, key) {
                    value = read(&toks, i + key.len());
                    // « 38,5 de fièvre »: the value before the word.
                    if value.is_none()
                        && name == "temperature"
                        && key[0] != "temperature"
                        && key[0] != "temp"
                    {
                        let mut j = i;
                        if j > 0
                            && matches!(&toks[j - 1], Tok::Word(w) if ["de", "of", "a", "avec", "with"].contains(&w.as_str()))
                        {
                            j -= 1;
                        }
                        if j > 0 {
                            value = temperature_value(&toks, j - 1);
                        }
                    }
                    if value.is_some() {
                        break 'scan;
                    }
                }
            }
        }
        // A temperature with its unit, without the word: « 38,5 °C ».
        if value.is_none() && name == "temperature" {
            for k in 0..toks.len() {
                if matches!(toks.get(k + 1), Some(Tok::Sym('°')))
                    && let Some(v) = temperature_value(&toks, k)
                    && v.1.is_some()
                {
                    value = Some(v);
                    break;
                }
            }
        }
        if let Some((v, unit)) = value {
            tags.push(format!("mesure/{name}"));
            fields.insert(name.to_string(), v);
            if let Some(u) = unit {
                fields.insert(format!("{name}_unit"), u.to_string());
            }
        }
    }
    Found { tags, fields }
}
