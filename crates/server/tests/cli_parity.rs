//! The command line has the same commands locally and through a node (docs/CLI.md): this fails when a command of the
//! usage text has no entry in the table, when an entry has no remote form and no reason, and when docs/CLI.md is not the
//! table.
#![allow(clippy::expect_used)]

use pensine_server::cli_table::{self, COMMANDS, Remote};
use pensine_server::commands::USAGE;

#[test]
fn every_command_of_the_usage_text_is_in_the_table_with_a_remote_form_or_a_reason() {
    let words = cli_table::usage_commands(USAGE);
    assert!(words.len() > 30, "the usage text was not read: {words:?}");
    for w in &words {
        assert!(
            COMMANDS
                .iter()
                .any(|c| c.words.split(' ').next() == Some(w)),
            "`{w}` is in the usage text and not in crates/server/src/cli_table.rs: give it a remote form, or the reason \
             it only works on the machine itself"
        );
    }
    for c in COMMANDS {
        if let Remote::Local(why) = c.remote {
            assert!(
                why.split(' ').count() >= 3,
                "`{}` has no remote form and no reason",
                c.words
            );
        }
        let first = c.words.split(' ').next().expect("a word");
        assert!(
            words.iter().any(|w| w == first) || matches!(first, "duplicates" | "vault"),
            "`{}` is in the table and not in the usage text",
            c.words
        );
        // Found by its own words, and by those only.
        let pos: Vec<String> = c.words.split(' ').map(String::from).collect();
        assert_eq!(cli_table::find(&pos).expect("found").words, c.words);
    }
}

#[test]
fn every_remote_command_is_a_command_the_local_line_knows() {
    // The word is read by the local dispatch (an error about missing words is fine, "unknown command" is not).
    let dir = std::env::temp_dir().join(format!("memcp-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    pensine_core::Brain::init(&dir, &[], pensine_core::text::Language::English).expect("init");
    let settings = pensine_server::config::Settings::default();
    for c in COMMANDS {
        if !matches!(c.remote, Remote::Exec | Remote::Upload(_)) {
            continue;
        }
        // Commands that would do real work with no argument at all.
        if matches!(
            c.words,
            "backup" | "doctor" | "duplicates" | "status" | "catalogue"
        ) {
            continue;
        }
        let mut words: Vec<String> = c.words.split(' ').map(String::from).collect();
        words.push("zz".into());
        let a = pensine_server::commands::parse(&words).expect("parse");
        let (result, _, _) = pensine_server::out::capture(|| {
            pensine_server::commands::dispatch(&dir, &a, &settings, None)
        });
        if let Err((_, message)) = result {
            assert!(
                !message.starts_with("unknown command"),
                "`{}` is in the table, the local line does not know it",
                c.words
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_table_of_docs_cli_is_the_generated_one() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/CLI.md");
    // A checkout on Windows may turn the line ends into CRLF: the content is what counts.
    let doc = std::fs::read_to_string(&path)
        .expect("docs/CLI.md")
        .replace("\r\n", "\n");
    let (start, end) = ("<!-- table:start -->\n", "<!-- table:end -->");
    let from = doc.find(start).expect("table:start") + start.len();
    let to = doc.find(end).expect("table:end");
    let table = cli_table::markdown();
    if std::env::var_os("MEMCP_BLESS").is_some() {
        let new = format!("{}{table}\n{}", &doc[..from], &doc[to..]);
        std::fs::write(&path, new).expect("write");
        return;
    }
    assert_eq!(
        doc[from..to].trim(),
        table.trim(),
        "docs/CLI.md is not the table of crates/server/src/cli_table.rs: MEMCP_BLESS=1 cargo test -p pensine-server --test cli_parity"
    );
}
