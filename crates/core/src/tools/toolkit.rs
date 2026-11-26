//! The toolkits of the professions: `skills` (what an assistant can do in a space, by profile) and, for health, the tools
//! a nurse uses (`target_set`, `signs_review`, `sign_set`). They are the same mechanisms the apps use, so any
//! assistant does what the app does.

use super::*;
use pensine_health::{Lang, Manifestation, builtin};

/// The kind of a space, as the memory knows it.
fn kind_of(brain: &Brain, space: &Space) -> String {
    brain.space_kind(space).unwrap_or_default()
}

fn health_space(
    brain: &Brain,
    caller: &Caller,
    args: &Value,
    write: bool,
) -> Result<Space, ToolError> {
    let space = if write {
        match args.get("space").and_then(Value::as_str) {
            Some(_) => write_space(brain, caller, args)?,
            None => caller
                .writable()
                .into_iter()
                .find(|s| kind_of(brain, s) == "health")
                .ok_or_else(|| ToolError::Invalid("give \"space\": a health space".into()))?,
        }
    } else {
        match args.get("space").and_then(Value::as_str) {
            Some(_) => read_spaces(caller, args)?
                .into_iter()
                .next()
                .ok_or_else(|| ToolError::Invalid("give \"space\": a health space".into()))?,
            None => caller
                .readable()
                .into_iter()
                .find(|s| kind_of(brain, s) == "health")
                .ok_or_else(|| ToolError::Invalid("give \"space\": a health space".into()))?,
        }
    };
    if kind_of(brain, &space) != "health" {
        return Err(ToolError::Invalid(format!("{space} is not a health space")));
    }
    Ok(space)
}

/// Folds a text for comparison: lower case, no accent.
fn fold(s: &str) -> String {
    crate::autotag::words(s).join(" ")
}

/// The sign of the referential that a word names: its id, its tag, or its French or English label.
fn sign_named(q: &str) -> Option<&'static Manifestation> {
    let want = fold(q);
    builtin().manifestations().iter().find(|m| {
        m.id == q
            || m.tag == q
            || fold(m.label(Lang::Fr)) == want
            || fold(m.label(Lang::En)) == want
    })
}

/// A refusal that helps: the unknown sign, and the closest ones of the referential.
fn unknown_sign(q: &str) -> ToolError {
    let close: Vec<String> =
        builtin().compact(Some("manifestations"), Some(q), Lang::Auto)["manifestations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str().map(String::from))
            .take(6)
            .collect();
    ToolError::Invalid(format!(
        "{q:?} is not a sign of the shared referential{}. Look it up with `health_referential`.",
        if close.is_empty() {
            String::new()
        } else {
            format!(" (close: {})", close.join(", "))
        }
    ))
}

fn signs_arg(args: &Value, key: &str) -> Result<Vec<&'static Manifestation>, ToolError> {
    let mut out: Vec<&'static Manifestation> = vec![];
    for v in args
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let q = v.as_str().ok_or_else(|| {
            ToolError::Invalid(format!("{key}: signs of the referential (texts)"))
        })?;
        let m = sign_named(q).ok_or_else(|| unknown_sign(q))?;
        if !out.iter().any(|x| x.id == m.id) {
            out.push(m);
        }
    }
    Ok(out)
}

/// First words that state what is aimed at (an infinitive, a stop): they belong to the precision, not to the title.
const AIMS: [&str; 34] = [
    "arreter",
    "reduire",
    "diminuer",
    "ameliorer",
    "augmenter",
    "retrouver",
    "gerer",
    "eviter",
    "limiter",
    "stopper",
    "perdre",
    "gagner",
    "reprendre",
    "controler",
    "supprimer",
    "calmer",
    "soigner",
    "guerir",
    "traiter",
    "retablir",
    "regler",
    "corriger",
    "stop",
    "quit",
    "reduce",
    "improve",
    "stopping",
    "avoid",
    "lose",
    "gain",
    "manage",
    "control",
    "fix",
    "treat",
];

/// The title of a target as a nurse writes it: the problem in one or two words (up to five), never an aim.
fn check_problem(problem: &str) -> Result<String, ToolError> {
    let p = problem.trim();
    let words: Vec<String> = crate::autotag::words(p);
    if p.is_empty() || words.is_empty() {
        return Err(ToolError::Invalid(
            "problem: the problem in one or two words (« Sommeil », « Achats impulsifs »)".into(),
        ));
    }
    if p.chars().count() > 60 || p.split_whitespace().count() > 5 {
        return Err(ToolError::Invalid(
            "problem: one or two words, five at most; the rest goes in `precision`".into(),
        ));
    }
    if AIMS.contains(&words[0].as_str()) {
        return Err(ToolError::Invalid(
            "problem: name the problem, not what is aimed at (« Achats impulsifs », not « Arrêter d'acheter »); put the aim in `precision` (« arrêter, <raison> »)".into(),
        ));
    }
    Ok(p.to_string())
}

pub(super) fn skills(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let profile = args.get("profile").and_then(Value::as_str);
    let mut out = vec![];
    for space in read_spaces(caller, args)? {
        let kind = kind_of(brain, &space);
        let p = crate::profession::json_for(Some(brain.dir()), &kind, space.as_str(), profile);
        if p.is_null() {
            continue;
        }
        out.push(json!({ "space": space.as_str(), "kind": kind, "profession": p }));
    }
    let mut answer = json!({ "notice": NOTICE, "spaces": out });
    // The person's keys (all spaces, not read-only) also see what waits for the person's decision.
    if caller.all_spaces && !caller.read_only_forced {
        answer["proposals_waiting"] = json!(crate::profession::pending(brain.dir()));
    }
    Ok(answer)
}

pub(super) fn profession_propose(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    if !caller.all_spaces || caller.read_only_forced {
        return Err(ToolError::Denied(
            "a profession is proposed with a key that reaches every space and may write".into(),
        ));
    }
    let profile = args
        .get("profile")
        .filter(|p| p.is_object())
        .ok_or_else(|| ToolError::Invalid("profile: an object (see `skills`)".into()))?;
    let kept = crate::profession::propose(brain.dir(), profile).map_err(ToolError::Invalid)?;
    Ok(json!({
        "result": "proposed",
        "profile": kept,
        "next": format!(
            "Nothing changes yet: a profession is an instruction, so the person accepts it themselves, with `memcp professions accept {}/{}` on the machine of the node.",
            kept["kind"].as_str().unwrap_or(""),
            kept["id"].as_str().unwrap_or("")
        ),
        "notice": NOTICE,
    }))
}

pub(super) fn target_set(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let space = health_space(brain, caller, args, true)?;
    let id = args.get("id").and_then(Value::as_str);
    let text = if id.is_none() || args.get("problem").is_some() {
        let problem = check_problem(str_arg(args, "problem")?)?;
        let precision = args
            .get("precision")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty());
        Some(match precision {
            Some(p) => format!("{problem} — {p}"),
            None => problem,
        })
    } else {
        None
    };
    let mut card = json!({});
    if let Some(text) = &text {
        card["text"] = json!(text);
    }
    for key in ["signs", "factors"] {
        if let Some(values) = args.get(key) {
            let values = values
                .as_array()
                .ok_or_else(|| ToolError::Invalid(format!("{key}: expected a list of texts")))?;
            let mut entries = Vec::new();
            for value in values {
                let value = value
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        ToolError::Invalid(format!("{key}: expected a non-empty text"))
                    })?;
                let entry = if key == "signs" {
                    sign_named(value).map(|m| m.id.as_str()).unwrap_or(value)
                } else {
                    value
                };
                let slug = sign_slug(entry);
                facts::normalize_tag(&slug)?;
                entries.push(entry);
            }
            card[key] = json!(entries);
        }
    }
    if let Some(col) = args.get("column") {
        card["column"] = col.clone();
    }
    let signs = card["signs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| match sign_named(s) {
            Some(m) => json!({ "id": m.id, "label": m.label(Lang::Fr) }),
            None => json!({ "id": sign_slug(s), "label": s }),
        })
        .collect::<Vec<_>>();
    let factors = card["factors"].clone();
    let call = if let Some(id) = id {
        card["id"] = json!(id);
        json!({ "space": space.as_str(), "page": "targets", "update": [card] })
    } else {
        json!({ "space": space.as_str(), "page": "targets", "add": [card] })
    };
    let done = run(brain, caller, "living_set", &call)?;
    let card = if id.is_some() {
        if done["updated"].as_u64().unwrap_or(0) == 0 {
            return Err(ToolError::Invalid(
                "id: no target with this id in the space".into(),
            ));
        }
        json!(id)
    } else {
        done["added"][0].clone()
    };
    Ok(json!({
        "result": if id.is_some() { "updated" } else { "created" },
        "space": space.as_str(),
        "card": card,
        "text": text,
        "signs": signs,
        "factors": factors,
        "notice": NOTICE,
    }))
}

pub(super) fn signs_review(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let space = health_space(brain, caller, args, false)?;
    let conn = brain.open_space(&space)?;
    let limit = args
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50)
        .clamp(1, 200);
    let since = match args.get("since").and_then(Value::as_str) {
        Some(d) => parse_date(&Value::String(d.to_string()))?,
        None => crate::util::now() - 14 * 86_400,
    };
    let only = args
        .get("sign")
        .and_then(Value::as_str)
        .map(|q| sign_named(q).ok_or_else(|| unknown_sign(q)))
        .transpose()?;
    let mut st = conn
        .prepare(
            "SELECT f.id FROM facts f WHERE f.status = 'active' AND f.created_at >= ?1
             AND f.id IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'signe/%' OR tag LIKE 'signe-refuse/%')
             AND f.id NOT IN (SELECT fact_id FROM fact_tags WHERE tag LIKE 'living/%' OR tag LIKE 'living-meta/%')
             ORDER BY f.created_at DESC, f.id DESC LIMIT ?2",
        )
        .map_err(|e| ToolError::Invalid(e.to_string()))?;
    let ids: Vec<String> = st
        .query_map(rusqlite::params![since, limit * 4], |r| r.get(0))
        .map_err(|e| ToolError::Invalid(e.to_string()))?
        .flatten()
        .collect();
    let mut notes = vec![];
    for id in ids {
        let Some(f) = facts::read_fact(&conn, &id)? else {
            continue;
        };
        let tags: Vec<&String> = f.tags.iter().filter(|t| t.starts_with("signe/")).collect();
        // What the person took off this note: the sweep leaves those signs alone, and `sign_set` puts one back.
        let refused: Vec<&str> = f
            .tags
            .iter()
            .filter_map(|t| t.strip_prefix(pensine_health::REFUSED_SIGN_PREFIX))
            .collect();
        if let Some(m) = only
            && !tags
                .iter()
                .any(|t| t.as_str() == format!("signe/{}", m.tag))
            && !refused.iter().any(|t| *t == m.tag)
        {
            continue;
        }
        // What the referential finds in the words of the note now: the words that bring each sign up.
        let found = pensine_health::detect(&f.text, Lang::Auto);
        let evidence: Vec<Value> = found
            .signs
            .iter()
            .filter(|s| {
                tags.iter()
                    .any(|t| t.as_str() == format!("signe/{}", s.tag))
            })
            .map(|s| json!({ "sign": s.id, "label": s.label, "words": s.words }))
            .collect();
        notes.push(json!({
            "id": f.id,
            "at": crate::util::iso(f.created_at),
            "author": f.author,
            "text": f.text.chars().take(280).collect::<String>(),
            "signs": tags.iter().filter_map(|t| t.strip_prefix("signe/")).collect::<Vec<_>>(),
            "refused": refused,
            "evidence": evidence,
            "from_words": !evidence.is_empty(),
        }));
        if notes.len() as i64 >= limit {
            break;
        }
    }
    Ok(json!({
        "notice": NOTICE,
        "space": space.as_str(),
        "since": crate::util::iso(since),
        "notes": notes,
        "how_to_correct": "Call `sign_set` with the note's id: `add` the sign that is missing, `remove` the one that is wrong.",
    }))
}

pub(super) fn sign_set(
    brain: &Brain,
    caller: &Caller,
    _name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let id = str_arg(args, "id")?;
    let add = signs_arg(args, "add")?;
    let remove = signs_arg(args, "remove")?;
    if add.is_empty() && remove.is_empty() {
        return Err(ToolError::Invalid(
            "add or remove: at least one sign of the referential".into(),
        ));
    }
    let tag = |m: &&Manifestation| json!(format!("signe/{}", m.tag));
    let mut call = json!({
        "id": id,
        "add": add.iter().map(tag).collect::<Vec<_>>(),
        "remove": remove.iter().map(tag).collect::<Vec<_>>(),
        "reason": args.get("reason").and_then(Value::as_str).unwrap_or("a sign corrected"),
    });
    // A correction by the person (or on their word) teaches the node; an engine proposing says `learn: false`.
    if let Some(l) = args.get("learn") {
        call["learn"] = l.clone();
    }
    run(brain, caller, "tags_set", &call)
}
