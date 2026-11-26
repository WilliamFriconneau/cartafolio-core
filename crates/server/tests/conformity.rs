//! Small checks of what the documents require (docs/CONFORMITY.md lists them all): discovery says nothing of what a
//! node holds, a join request lives two minutes, a token is kept only as a digest, and no screen holds a forbidden word.

use std::error::Error;
use std::fs;

use pensine_core::access::{self, Rights};
use pensine_core::text::Language;
use pensine_core::{Brain, Space};
use pensine_server::app::App;
use pensine_server::http::Request;
use serde_json::{Value, json};

type Res = Result<(), Box<dyn Error>>;

fn temp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("memcp-conf-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    d
}

fn app(tag: &str) -> Result<(App, std::path::PathBuf), Box<dyn Error>> {
    let dir = temp(tag);
    let brain = Brain::init(&dir, &[Space::new("notes")?], Language::English)?;
    Ok((App::new(brain), dir))
}

fn request(method: &str, path: &str, body: Value) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        query: Default::default(),
        headers: Default::default(),
        body: body.to_string().into_bytes(),
        ip: Some("192.168.1.30".into()),
        via_ingress: false,
        tailnet_login: None,
    }
}

/// DISCOVERY.md: advertising says that a node is here, and nothing of what it holds.
#[test]
fn discovery_tells_the_service_the_name_and_the_version_and_nothing_else() -> Res {
    let (mut a, dir) = app("wellknown")?;
    a.node_name = "kitchen".into();
    let r = a.handle(&request("GET", "/.well-known/pensine", json!({})));
    assert_eq!(r.status, 200);
    for path in ["/.well-known/silice", "/.well-known/memcp"] {
        let alias = a.handle(&request("GET", path, json!({})));
        assert_eq!(alias.status, r.status);
        assert_eq!(alias.headers, r.headers);
        assert_eq!(alias.body, r.body);
    }
    let v = r.body_json();
    let mut keys: Vec<&str> = v
        .as_object()
        .ok_or("object")?
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "id",
            "join",
            "name",
            "product",
            "service",
            "version",
            "view_contract"
        ]
    );
    assert_eq!(
        v["id"].as_str().map(str::len),
        Some(14),
        "the id names the node, nothing more"
    );
    assert_eq!(v["service"], "memcp");
    assert_eq!(v["product"], "pensine");
    assert_eq!(v["name"], "kitchen");
    let _ = fs::remove_dir_all(dir);
    Ok(())
}

/// DISCOVERY.md: a request lives two minutes.
#[test]
fn a_join_request_lives_two_minutes() -> Res {
    let (a, dir) = app("join")?;
    let r = a.handle(&request("POST", "/join", json!({ "device": "Laptop" })));
    assert_eq!(r.body_json()["expires_in"], 120);
    let _ = fs::remove_dir_all(dir);
    Ok(())
}

/// SECURITY.md: tokens are shown once; only their digest is stored.
#[test]
fn a_token_is_kept_only_as_a_digest() -> Res {
    let (a, dir) = app("digest")?;
    let ctl = a.brain.open_control()?;
    let token = access::connect(
        &ctl,
        "claude",
        &Rights::parse("notes:rw")?,
        None,
        false,
        false,
    )?;
    drop(ctl);
    let mut found = 0;
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = fs::read(&p) {
                let needle = token.as_bytes();
                if bytes.windows(needle.len()).any(|w| w == needle) {
                    found += 1;
                }
            }
        }
    }
    assert_eq!(found, 0, "the token is written on disk");
    let _ = fs::remove_dir_all(dir);
    Ok(())
}

/// UI-SPEC.md, acceptance 4: no screen contains *bridge*, *capture program*, *capture app*, *proxy* or *vault*. The text of the page (what is not a
/// script, a style or a tag) and every sentence of its translations are checked.
#[test]
fn no_screen_holds_a_forbidden_word() -> Res {
    let html = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/admin.html"))?;
    let mut text = String::new();
    let mut rest = html.as_str();
    while let Some(i) = rest.find('<') {
        text.push_str(&rest[..i]);
        rest = &rest[i..];
        let after = |tag: &str, s: &str| s.find(tag).map(|j| j + tag.len());
        let skip = if rest.starts_with("<script") {
            after("</script>", rest)
        } else if rest.starts_with("<style") {
            after("</style>", rest)
        } else {
            after(">", rest)
        };
        rest = &rest[skip.unwrap_or(rest.len())..];
        text.push(' ');
    }
    text.push_str(rest);
    // The translation table: `"English sentence": "phrase"` lines.
    for line in html
        .lines()
        .filter(|l| l.trim_start().starts_with('"') && l.contains("\": \""))
    {
        text.push('\n');
        text.push_str(line);
    }
    let low = text.to_lowercase();
    for w in [
        "bridge",
        "capture program",
        "capture app",
        "proxy",
        "proxies",
        "vault",
    ] {
        assert!(!low.contains(w), "the page says {w:?}");
    }
    Ok(())
}
