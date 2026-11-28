use super::ACCOUNTS;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

#[derive(Default)]
pub struct Audit(Mutex<HashSet<(usize, bool, String)>>);

impl Audit {
    pub fn observe(&self, value: &Value, account: usize, scoped: bool) {
        let mut ids = Vec::new();
        collect_ids(value, &mut ids);
        self.0
            .lock()
            .unwrap()
            .extend(ids.into_iter().map(|id| (account, scoped, id.into())));
    }

    pub fn verify(&self, dir: &Path) {
        let rows = ACCOUNTS.map(|account| {
            let db = Connection::open(dir.join("accounts").join(account).join("store.db")).unwrap();
            let mut query = db
                .prepare("SELECT id, space FROM facts UNION ALL SELECT id, space FROM files")
                .unwrap();
            query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<HashMap<_, _>>>()
                .unwrap()
        });
        for (account, scoped, id) in self.0.lock().unwrap().iter() {
            assert!(
                !rows[1 - account].contains_key(id),
                "foreign id returned: {id}"
            );
            let space = rows[*account]
                .get(id)
                .unwrap_or_else(|| panic!("unknown id returned: {id}"));
            if *scoped {
                assert_eq!(space, "shared", "scoped client saw {id}");
            }
        }
    }
}

fn collect_ids<'a>(value: &'a Value, ids: &mut Vec<&'a str>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| collect_ids(item, ids)),
        Value::Object(fields) => {
            for (key, item) in fields {
                if matches!(key.as_str(), "id" | "replaces" | "todo" | "note") {
                    if let Some(id) = item.as_str() {
                        ids.push(id);
                    }
                }
                collect_ids(item, ids);
            }
        }
        _ => {}
    }
}
