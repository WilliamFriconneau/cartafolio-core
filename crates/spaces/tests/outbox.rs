//! The deliveries of a kept note.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_spaces::*;

fn connector(id: &str, kind: ConnectorKind, icloud: bool) -> ConnectorConfig {
    ConnectorConfig {
        id: id.into(),
        kind,
        mode: Mode::Export,
        target: String::new(),
        icloud,
    }
}

fn space(id: &str, sensitive: bool, connectors: Vec<ConnectorConfig>) -> Space {
    Space {
        sensitive,
        connectors,
        ..Space::default_notes(id)
    }
    .with_path(id)
}

trait WithPath {
    fn with_path(self, p: &str) -> Self;
}
impl WithPath for Space {
    fn with_path(mut self, p: &str) -> Self {
        self.id = p.into();
        self.path = p.into();
        self
    }
}

#[test]
fn one_delivery_per_space_and_connector_never_twice_never_local() {
    let t = Tree::new(vec![
        space(
            "sante",
            true,
            vec![
                connector("dossier", ConnectorKind::Markdown, false),
                connector("ici", ConnectorKind::Local, false),
            ],
        ),
        space(
            "travail",
            false,
            vec![
                connector("drive", ConnectorKind::Drive, false),
                connector("mem0", ConnectorKind::McpMemory, false),
            ],
        ),
    ])
    .unwrap();
    let p = plan(
        &t,
        "n1",
        &[
            "travail".into(),
            "sante".into(),
            "travail".into(),
            "inconnu".into(),
        ],
        false,
    );
    let keys: Vec<&str> = p.iter().map(|x| x.key.as_str()).collect();
    assert_eq!(
        keys,
        ["n1/travail/drive", "n1/travail/mem0", "n1/sante/dossier"]
    );
}

#[test]
fn a_distress_note_has_no_delivery() {
    // law: S1
    let t = Tree::new(vec![space(
        "travail",
        false,
        vec![connector("drive", ConnectorKind::Drive, false)],
    )])
    .unwrap();
    assert!(plan(&t, "n1", &["travail".into()], true).is_empty());
}

#[test]
fn a_sensitive_space_refuses_icloud() {
    let n = space(
        "sante",
        true,
        vec![connector("dossier", ConnectorKind::Markdown, true)],
    );
    assert_eq!(Tree::check(&n), Err(SpaceError::SensitiveOnIcloud));
    let ok = space(
        "perso",
        false,
        vec![connector("dossier", ConnectorKind::Markdown, true)],
    );
    assert_eq!(Tree::check(&ok), Ok(()));
}

#[test]
fn retries_wait_longer_up_to_six_hours() {
    assert_eq!(retry_after_seconds(0), 60);
    assert_eq!(retry_after_seconds(1), 120);
    assert_eq!(retry_after_seconds(30), 6 * 3600);
}
