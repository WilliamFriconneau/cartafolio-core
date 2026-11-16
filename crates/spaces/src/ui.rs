//! The settings screens, described once as data (`ui/screens.json`) with
//! their words in French, English and Spanish (`ui/texts.json`). Every
//! platform draws them natively from this description; the outline is what a
//! renderer is tested against, so none drops a field or changes the order.
//! Capture (microphone, notes, watch) is not described here.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const SCREENS_JSON: &str = include_str!("../ui/screens.json");
const TEXTS_JSON: &str = include_str!("../ui/texts.json");

pub const LANGUAGES: [&str; 3] = ["fr", "en", "es"];
pub const PLATFORMS: [&str; 5] = ["ios", "macos", "windows", "linux", "haos"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Secret,
    Path,
    Switch,
    Choice,
    /// A list of words.
    Words,
    /// Several spaces to pick.
    Spaces,
    Connectors,
    SpaceTree,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub max_len: Option<u32>,
    #[serde(default)]
    pub options: Vec<String>,
    /// Shown only when this switch is on, or `field=value` holds.
    #[serde(default)]
    pub when: Option<String>,
    /// Where the option works; empty: everywhere. Elsewhere it is shown
    /// switched off, with the reason.
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub id: String,
    pub sections: Vec<Section>,
}

#[derive(Deserialize)]
struct Screens {
    screens: Vec<Screen>,
}

pub fn screens() -> &'static [Screen] {
    static S: OnceLock<Vec<Screen>> = OnceLock::new();
    S.get_or_init(|| {
        // Shipped with the crate and checked by its tests; never empty in a release.
        serde_json::from_str::<Screens>(SCREENS_JSON)
            .map(|s| s.screens)
            .unwrap_or_default()
    })
}

pub fn screen(id: &str) -> Option<&'static Screen> {
    screens().iter().find(|s| s.id == id)
}

fn texts() -> &'static BTreeMap<String, BTreeMap<String, String>> {
    static T: OnceLock<BTreeMap<String, BTreeMap<String, String>>> = OnceLock::new();
    T.get_or_init(|| serde_json::from_str(TEXTS_JSON).unwrap_or_default())
}

/// The text of `key` in `lang` (French when the language is unknown; the key
/// itself when missing, which a test forbids).
pub fn text(key: &str, lang: &str) -> String {
    let all = texts();
    all.get(lang)
        .or_else(|| all.get("fr"))
        .and_then(|t| t.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// Every text key a screen uses: its title, each section's title, each
/// field's label and help, each choice, each reason an option is off.
pub fn text_keys(screen: &Screen) -> Vec<String> {
    let mut keys = vec![format!("{}.title", screen.id)];
    for s in &screen.sections {
        keys.push(format!("{}.{}.title", screen.id, s.id));
        for f in &s.fields {
            keys.push(format!("{}.{}.label", screen.id, f.id));
            keys.push(format!("{}.{}.help", screen.id, f.id));
            for o in &f.options {
                keys.push(format!("{}.{}.{}", screen.id, f.id, o));
            }
            if !f.platforms.is_empty() {
                keys.push(format!("{}.{}.unavailable", screen.id, f.id));
            }
        }
    }
    keys
}

/// What a renderer must show, in order: `section:<id>` then `field:<id>:<kind>`.
pub fn outline(screen: &Screen) -> Vec<String> {
    let mut out = vec![];
    for s in &screen.sections {
        out.push(format!("section:{}", s.id));
        for f in &s.fields {
            let kind = serde_json::to_value(&f.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            out.push(format!("field:{}:{kind}", f.id));
        }
    }
    out
}

/// None when the option works on `platform`; else the reason, in `lang`.
pub fn unavailable(screen: &Screen, field: &Field, platform: &str, lang: &str) -> Option<String> {
    (!field.platforms.is_empty() && !field.platforms.iter().any(|p| p == platform))
        .then(|| text(&format!("{}.{}.unavailable", screen.id, field.id), lang))
}

/// Checks a value for a field: required, length. The reason in `lang`.
pub fn validate(field: &Field, value: &str, lang: &str) -> Option<String> {
    if field.required && value.trim().is_empty() {
        return Some(text("common.required", lang));
    }
    if let Some(max) = field.max_len
        && value.chars().count() > max as usize
    {
        return Some(text("common.too_long", lang).replace("{max}", &max.to_string()));
    }
    None
}
