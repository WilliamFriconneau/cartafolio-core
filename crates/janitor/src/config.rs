//! The janitor's settings: a file of `key = value` lines (`#` starts a comment), read once. Nothing secret is in
//! it: a key is a file, named here, that only the janitor reads.

use std::path::{Path, PathBuf};

/// No `Debug`: it holds keys.
#[derive(Clone)]
pub struct Config {
    /// The address of the janitor's connection: `https://<server>/mcp/<name>`.
    pub url: String,
    /// The connection's key, read from `key_file`.
    pub key: String,
    /// The spaces whose inbox the janitor files. Empty (the default): every space its key reaches. « À trier » is always
    /// read: it is where the devices write, and no space is a default.
    pub inbox_vaults: Vec<String>,
    /// Words that mark a note as being about the person's work (a product, a company, a colleague's team): such a note
    /// goes to the space `work` from « À trier » before any model. Empty by default: it is the person's own list.
    pub work_words: Vec<String>,
    pub utc_offset_minutes: i32,
    /// The models to try, in order: `local` (Ollama, on this machine or another) and `api` (any OpenAI-compatible service, only when named).
    pub models: Vec<String>,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub api_model: Option<String>,
    pub ollama_url: String,
    pub ollama_model: String,
    pub state_dir: PathBuf,
    /// The most notes filed in one pass.
    pub max_notes: usize,
    /// The frequency rule: a word found in at least this many notes of one space (and far less elsewhere) becomes a
    /// keyword of that space without asking anybody.
    pub keyword_min_notes: usize,
    /// The least minutes between two times the model is asked to propose keywords (a night pass ignores it).
    pub keywords_every_minutes: u64,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn path(value: &str) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(value),
    }
}

fn read_key(file: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let key = text.trim().to_string();
    if key.is_empty() {
        return Err(format!("{} is empty", file.display()));
    }
    Ok(key)
}

/// Minutes east of UTC, from the system's `date +%z`.
fn system_offset() -> i32 {
    let out = std::process::Command::new("date").arg("+%z").output().ok();
    let z = out
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let z = z.trim();
    if z.len() != 5 {
        return 0;
    }
    let sign = if z.starts_with('-') { -1 } else { 1 };
    sign * (z[1..3].parse::<i32>().unwrap_or(0) * 60 + z[3..5].parse::<i32>().unwrap_or(0))
}

impl Config {
    pub fn load(file: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        Config::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let mut map = std::collections::BTreeMap::new();
        for (n, raw) in text.lines().enumerate() {
            // A comment starts a line, or follows a space: a `#` inside a value (a path) stays.
            let line = raw.trim();
            let line = if line.starts_with('#') {
                ""
            } else {
                line.find(" #").map_or(line, |at| &line[..at]).trim()
            };
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or(format!("line {}: `key = value` expected", n + 1))?;
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
        let get = |k: &str| map.get(k).cloned();
        let need = |k: &str| get(k).ok_or(format!("{k} is needed"));
        let known = [
            "url",
            "key_file",
            // No longer used: « À trier » replaced the default space. Accepted, so that an old file still works.
            "default_space",
            "inbox_vaults",
            "work_words",
            "utc_offset_minutes",
            "models",
            "api_url",
            "api_key_file",
            "api_model",
            "ollama_url",
            "ollama_model",
            "state_dir",
            "max_notes",
            "keyword_min_notes",
            "keywords_every_minutes",
        ];
        if let Some(bad) = map.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown setting {bad}"));
        }
        let number = |k: &str, d: i64| -> Result<i64, String> {
            get(k).map_or(Ok(d), |v| v.parse().map_err(|_| format!("{k}: a number")))
        };
        let c = Config {
            url: need("url")?,
            key: read_key(&path(&need("key_file")?))?,
            inbox_vaults: get("inbox_vaults").map_or_else(Vec::new, |v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
            work_words: get("work_words").map_or_else(Vec::new, |v| {
                v.split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            }),
            utc_offset_minutes: i32::try_from(number(
                "utc_offset_minutes",
                i64::from(system_offset()),
            )?)
            .map_err(|_| "utc_offset_minutes: minutes".to_string())?,
            models: get("models")
                .unwrap_or_else(|| "local".into())
                .split(',')
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
                .collect(),
            api_url: get("api_url"),
            api_key: match get("api_key_file") {
                Some(f) => Some(read_key(&path(&f))?),
                None => None,
            },
            api_model: get("api_model"),
            ollama_url: get("ollama_url").unwrap_or_else(|| "http://127.0.0.1:11436".into()),
            ollama_model: get("ollama_model").unwrap_or_else(|| "qwen3:8b".into()),
            state_dir: get("state_dir")
                .map_or_else(|| home().join(".local/share/memcp-janitor"), |v| path(&v)),
            max_notes: usize::try_from(number("max_notes", 30)?)
                .map_err(|_| "max_notes".to_string())?,
            keyword_min_notes: usize::try_from(number("keyword_min_notes", 10)?)
                .map_err(|_| "keyword_min_notes".to_string())?,
            keywords_every_minutes: u64::try_from(number("keywords_every_minutes", 360)?)
                .map_err(|_| "keywords_every_minutes".to_string())?,
        };
        if c.models.iter().any(|m| m == "api") && (c.api_url.is_none() || c.api_model.is_none()) {
            return Err("models names `api`: api_url and api_model are needed".into());
        }
        if c.models.is_empty() {
            return Err("models: at least one of `local`, `api`".into());
        }
        if let Some(bad) = c
            .models
            .iter()
            .find(|m| !["local", "api"].contains(&m.as_str()))
        {
            return Err(format!("models: {bad} is not `local` or `api`"));
        }
        if !memcp_client::allowed_url(&c.ollama_url) {
            return Err(format!(
                "ollama_url {} is refused: HTTPS, or in clear only on this machine or the own network",
                c.ollama_url
            ));
        }
        if let Some(url) = &c.api_url
            && !memcp_client::allowed_url(url)
        {
            return Err(format!(
                "api_url {url} is refused: HTTPS, or in clear only on this machine or the own network"
            ));
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::Config;

    fn settings(extra: &str) -> Result<Config, String> {
        let dir = std::env::temp_dir().join(format!("memcp-janitor-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("key"), "hcp_x\n").map_err(|e| e.to_string())?;
        Config::parse(&format!(
            "url = http://127.0.0.1:8791/mcp/janitor\nkey_file = {}\ndefault_space = personal\n{extra}",
            dir.join("key").display()
        ))
    }

    #[test]
    fn only_the_local_model_is_used_unless_an_api_is_named() {
        let c = settings("").expect("minimal settings");
        assert_eq!(c.models, ["local"]);
        assert!(c.inbox_vaults.is_empty(), "every space the key reaches");
        assert!(c.api_url.is_none());
        assert!(
            settings("models = api\n").is_err(),
            "api needs its address and model"
        );
        assert!(settings("models = api,local\napi_url = https://x.example/v1/chat/completions\napi_model = m\n").is_ok());
        assert!(
            settings("models = api\napi_url = http://example.com/v1\napi_model = m\n").is_err(),
            "no key in clear to the internet"
        );
        assert!(settings("models = airouter\n").is_err());
        assert!(
            settings("airouter_url = x\n").is_err(),
            "an unknown setting is refused"
        );
        assert_eq!(
            settings("inbox_vaults = health, work\n")
                .expect("two inboxes")
                .inbox_vaults,
            ["health", "work"]
        );
    }
}
