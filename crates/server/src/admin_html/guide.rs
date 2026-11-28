use super::{address_line, esc, hidden_field, Dashboard, Feedback};
use std::fmt::Write;

pub(super) fn render(view: &Dashboard<'_>, csrf: &str, address: Option<&str>) -> String {
    let t = view.texts;
    let assistant = match (view.field("assistant"), view.feedback) {
        (_, Feedback::Key(_)) | (Some("key"), _) => "key",
        (Some("chatgpt"), _) => "chatgpt",
        _ => "claude",
    };
    let mut choices = String::new();
    for (id, label) in [
        ("claude", t.claude),
        ("chatgpt", t.chatgpt),
        ("key", t.other),
    ] {
        choices.push_str(&radio(id, "assistant", id, label, assistant == id));
    }
    let reach = reach(view);
    let setup = setup(view, address);
    let name = esc(view.field("name").unwrap_or(""));
    format!(
        "<details open><summary>{}</summary><form id=\"guide-form\" method=\"post\" action=\"oauth\">{}\
         <ol><li><fieldset><legend>{}</legend>{choices}</fieldset></li>\
         <li><fieldset><legend>{}</legend>{reach}</fieldset></li>\
         <li><fieldset><legend>{}</legend>{setup}\
         <div class=\"key\"><label>{} <input name=\"name\" value=\"{name}\"></label> \
         <button formaction=\"connections\">{}</button></div>\
         <button class=\"oauth\">{}</button>{}</fieldset></li></ol></form></details>",
        t.guide, hidden_field("csrf", csrf), t.assistant, t.reach, t.in_assistant,
        t.name, t.create_key, t.create_code, feedback(view),
    )
}

fn reach(view: &Dashboard<'_>) -> String {
    let t = view.texts;
    let selected = view.field("scope") == Some("selected");
    let mut body = format!(
        "<p class=\"oauth\">{}</p>{}{}<div class=\"space-options\"><p>{}</p>",
        t.reach_help,
        radio("all", "scope", "all", t.all, !selected),
        radio("selected", "scope", "selected", t.selected, selected),
        t.children,
    );
    for space in view.spaces {
        let name = format!("s_{}", space.name);
        body.push_str(&checkbox(
            &esc(&name),
            &esc(&space.label),
            view.field(&name) == Some("1"),
        ));
    }
    body.push_str("</div>");
    body.push_str(&checkbox(
        "read_only",
        t.read_only,
        view.field("read_only") == Some("1"),
    ));
    body
}

fn setup(view: &Dashboard<'_>, address: Option<&str>) -> String {
    let t = view.texts;
    let mut setup = format!("<p class=\"key\">{}</p>", t.key_setup);
    if let Some(address) = address {
        let _ = write!(
            setup,
            "<div class=\"oauth\"><p>{}</p><p class=\"claude\">{}</p>\
             <p class=\"chatgpt\">{}</p></div>{}<p class=\"oauth\">{}</p>",
            t.public_access,
            t.claude_setup,
            t.chatgpt_setup,
            address_line(t.paste, address, t),
            t.enter_code,
        );
    }
    setup
}

fn feedback(view: &Dashboard<'_>) -> String {
    let t = view.texts;
    match view.feedback {
        Feedback::Code(code) => format!(
            "<div class=\"invitation\"><p role=\"status\">{}</p>\
             <input aria-label=\"{}\" id=\"invitation-code\" readonly value=\"{}\">\
             <button type=\"button\" data-copy=\"{}\">{}</button><p>{}</p></div>",
            t.code_expiry,
            t.one_time_code,
            esc(code),
            t.copied,
            t.copy_code,
            t.code_where
        ),
        Feedback::Key(key) => format!(
            "<p role=\"status\">{} <code>{}</code></p>",
            t.save_key,
            esc(key)
        ),
        _ => String::new(),
    }
}

fn radio(id: &str, name: &str, value: &str, label: &str, checked: bool) -> String {
    let checked = if checked { " checked" } else { "" };
    format!(
        "<label class=\"choice\"><input type=\"radio\" id=\"{id}\" name=\"{name}\" \
         value=\"{value}\"{checked}> {label}</label>"
    )
}

fn checkbox(name: &str, label: &str, checked: bool) -> String {
    let checked = if checked { " checked" } else { "" };
    format!(
        "<label class=\"choice\"><input type=\"checkbox\" name=\"{name}\" value=\"1\"{checked}> {label}</label>"
    )
}
