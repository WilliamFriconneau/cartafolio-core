use super::{esc, hidden_field, Dashboard};
use crate::texts::Texts;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn render(view: &Dashboard<'_>, csrf: &str) -> String {
    let t = view.texts;
    let notes: u64 = view.spaces.iter().map(|space| space.notes).sum();
    let files: u64 = view.spaces.iter().map(|space| space.files).sum();
    let mut spaces = String::new();
    for space in view.spaces {
        let _ = write!(
            spaces,
            "<li>{} · {} {}, {} {}</li>",
            esc(&space.label),
            space.notes,
            t.notes,
            space.files,
            t.files,
        );
    }
    format!(
        "<details><summary>{} ({})</summary>{}</details>\
         <details><summary>{} ({})</summary>{}</details>\
         <details><summary>{} ({})</summary><ul>{spaces}</ul></details>\
         <details><summary>{}: {}, {} {}</summary><p>{files} {} · {}: {} {}</p></details>",
        t.connections,
        view.connections.len(),
        connections(view, csrf),
        t.sessions,
        view.sessions.len(),
        sessions(view, csrf),
        t.spaces,
        view.spaces.len(),
        t.status,
        esc(view.version),
        notes,
        t.notes,
        t.files,
        t.disk,
        disk_used(view.data_dir),
        t.bytes,
    )
}

fn connections(view: &Dashboard<'_>, csrf: &str) -> String {
    let t = view.texts;
    let mut body = format!(
        "<form method=\"post\" action=\"alias\">{}<label>{} <input name=\"alias\" \
         value=\"{}\" required maxlength=\"32\"></label> <button>{}</button></form><ul>",
        hidden_field("csrf", csrf),
        t.alias,
        esc(view.alias),
        t.save_alias,
    );
    for connection in view.connections {
        let grants = connection
            .grants
            .iter()
            .map(|grant| {
                format!(
                    "{} ({})",
                    space_name(&grant.space, t),
                    mode(connection.read_only || !grant.write, t)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let revoke = revoke("connections/revoke", &connection.id, csrf, t);
        let _ = write!(
            body,
            "<li><strong>{}</strong> · {grants}{revoke}</li>",
            esc(&connection.name)
        );
    }
    body.push_str("</ul>");
    body
}

fn sessions(view: &Dashboard<'_>, csrf: &str) -> String {
    let t = view.texts;
    let mut body = format!("<p>{}</p><ul>", t.new_rights);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs() as i64);
    for session in view.sessions {
        let spaces = session
            .spaces
            .iter()
            .map(|space| space_name(space, t))
            .collect::<Vec<_>>()
            .join(", ");
        let days = (session.refresh_until.saturating_sub(now))
            .div_euclid(86_400)
            .max(0);
        let revoke = revoke("oauth/revoke", &session.id, csrf, t);
        let _ = write!(
            body,
            "<li><strong>{}</strong> ({}) · {} · {spaces} · {}: {days}{revoke}</li>",
            esc(&session.name),
            esc(&session.client),
            mode(session.read_only, t),
            t.expires,
        );
    }
    body.push_str("</ul>");
    body
}

fn mode(read_only: bool, t: &Texts) -> &str {
    if read_only {
        t.read_only
    } else {
        t.read_write
    }
}

fn space_name(name: &str, t: &Texts) -> String {
    if name == "*" {
        t.all.to_owned()
    } else {
        esc(name)
    }
}

fn revoke(action: &str, id: &str, csrf: &str, t: &Texts) -> String {
    format!(
        "<form method=\"post\" action=\"{action}\">{}{}<button>{}</button></form>",
        hidden_field("csrf", csrf),
        hidden_field("id", id),
        t.revoke,
    )
}

fn disk_used(path: &Path) -> u64 {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            if kind.is_dir() {
                Some(disk_used(&entry.path()))
            } else if kind.is_file() {
                Some(entry.metadata().ok()?.len())
            } else {
                None
            }
        })
        .sum()
}
