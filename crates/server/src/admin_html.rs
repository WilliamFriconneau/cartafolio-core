mod guide;
mod sections;

use crate::error::ServerError;
use crate::html::escape_html as esc;
use crate::oauth::SessionInfo;
use crate::texts::Texts;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use pensine_access::Connection;
use pensine_store::SpaceInfo;
use rand::{rngs::OsRng, TryRngCore};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;

pub(super) const CSRF_COOKIE: &str = "pensine_csrf";
const STYLE: &str = "
:root{color-scheme:light}
@media(prefers-color-scheme:dark){:root{color-scheme:dark}}
body{font:1rem system-ui,sans-serif;max-width:44rem;margin:1rem auto;padding:0 .8rem;overflow-wrap:anywhere}
h1{font-size:1.3rem;margin:0 0 .5rem}header{margin-bottom:1rem}
details{border-top:1px solid GrayText;padding:.65rem 0}summary{cursor:pointer;font-weight:600}
li,ol,form,p{margin:.4rem 0}ol,ul{padding-inline-start:1.4rem}
fieldset{border:0;margin:.35rem 0;padding:0;min-inline-size:0}label.choice{display:block}
input,button{font:inherit;max-width:100%;box-sizing:border-box}button{cursor:pointer}
.address{display:flex;gap:.4rem;align-items:center}.address input{flex:1;min-width:0}
.space-options,.key,.chatgpt{display:none}
form:has(#selected:checked) .space-options,form:has(#key:checked) .key,form:has(#chatgpt:checked) .chatgpt{display:block}
form:has(#key:checked) .oauth,form:not(:has(#claude:checked)) .claude{display:none}
";
const SCRIPT: &str = r#"
document.querySelectorAll('[data-copy]').forEach(button => {
  button.addEventListener('click', async () => {
    const input = button.previousElementSibling;
    if (navigator.clipboard) {
      try {
        await navigator.clipboard.writeText(input.value);
        button.textContent = button.dataset.copy;
        return;
      } catch {}
    }
    input.focus();
    input.select();
  });
});
const guide = document.querySelector('#guide-form');
const connectionName = guide.elements.namedItem('name');
function updateName() {
  connectionName.disabled = !document.querySelector('#key').checked;
  connectionName.required = !connectionName.disabled;
}
guide.addEventListener('change', updateName);
updateName();
"#;

pub(super) struct Dashboard<'a> {
    pub user: &'a str,
    pub texts: &'a Texts,
    pub public_url: Option<&'a str>,
    pub alias: &'a str,
    pub connections: &'a [Connection],
    pub sessions: &'a [SessionInfo],
    pub spaces: &'a [SpaceInfo],
    pub version: &'a str,
    pub csrf: &'a str,
    pub secure_cookie: bool,
    pub data_dir: &'a Path,
    pub fields: Option<&'a HashMap<String, String>>,
    pub feedback: Feedback<'a>,
}

#[derive(Clone, Copy)]
pub(super) enum Feedback<'a> {
    None,
    Key(&'a str),
    Code(&'a str),
    Error(&'a str),
}

impl Dashboard<'_> {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields?.get(name).map(String::as_str)
    }
}

pub(super) fn dashboard(view: Dashboard<'_>) -> Result<Response, ServerError> {
    let csrf = view.csrf;
    let nonce = random_token()?;
    let t = view.texts;
    let address = view
        .public_url
        .map(|url| format!("{}/mcp/{}", url.trim_end_matches('/'), view.alias));
    let header_address = address.as_ref().map_or_else(
        || format!("<p>{}</p>", t.no_public),
        |url| address_line(t.address, url, t),
    );
    let guide = guide::render(&view, csrf, address.as_deref());
    let sections = sections::render(&view, csrf);
    let error = match view.feedback {
        Feedback::Error(message) => format!("<p role=\"alert\">{message}</p>"),
        _ => String::new(),
    };
    let html = format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title><style nonce=\"{nonce}\">{STYLE}</style></head>\
         <body><header><h1>{} · {}</h1>{header_address}</header>\
         <main>{error}{guide}{sections}</main><script nonce=\"{nonce}\">{SCRIPT}</script></body></html>",
        t.lang, t.product, t.product, esc(view.user),
    );
    let status = match view.feedback {
        Feedback::Error(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::OK,
    };
    let mut response = (status, Html(html)).into_response();
    let headers = response.headers_mut();
    let secure = if view.secure_cookie { "; Secure" } else { "" };
    headers.insert(
        header::SET_COOKIE,
        header_value(&format!(
            "{CSRF_COOKIE}={csrf}; Path=/; HttpOnly; SameSite=Strict{secure}"
        ))?,
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        header_value(&format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; base-uri 'none'; frame-ancestors 'self'"
    ))?,
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}

fn header_value(value: &str) -> Result<HeaderValue, ServerError> {
    HeaderValue::from_str(value).map_err(|error| std::io::Error::other(error).into())
}

fn address_line(label: &str, address: &str, t: &Texts) -> String {
    format!(
        "<div class=\"address\"><span>{label}</span><input aria-label=\"{label}\" readonly value=\"{}\">\
         <button type=\"button\" data-copy=\"{}\">{}</button></div>",
        esc(address),
        t.copied,
        t.copy
    )
}

fn hidden_field(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
        esc(value)
    )
}

fn random_token() -> Result<String, ServerError> {
    let mut bytes = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(std::io::Error::other)?;
    let mut token = String::with_capacity(64);
    for byte in bytes {
        write!(token, "{byte:02x}").map_err(std::io::Error::other)?;
    }
    Ok(token)
}

pub(super) fn csrf_matches(
    headers: &HeaderMap,
    fields: &HashMap<String, String>,
    expected: &str,
) -> bool {
    match (
        cookie_value(headers, CSRF_COOKIE),
        fields.get("csrf").map(String::as_str),
    ) {
        (Some(cookie), Some(field)) => bool::from(
            subtle::ConstantTimeEq::ct_eq(cookie.as_bytes(), expected.as_bytes())
                & subtle::ConstantTimeEq::ct_eq(field.as_bytes(), expected.as_bytes()),
        ),
        _ => false,
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.trim().to_owned())
    })
}
