mod audit;
mod client;
#[cfg(unix)]
pub mod crash;
mod invariant;
mod oauth;
pub mod workload;

use client::agent;
pub use client::Client;
use client::Reply;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use pensine_access::{Accounts, Grant};
use pensine_server::{Config, Handle, Server};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Barrier;
use std::thread;

pub const MISSING: &str = "not found";
pub const CONFLICT: &str = "Conflict";
pub const PERMANENT: &str = "the todo note of a space is permanent: update it with supersede";
pub const ISSUER: &str = "https://server.example.test";
pub const ACCOUNTS: [&str; 2] = ["amber", "violet"];

pub struct AccountData {
    keys: Vec<String>,
    pub hidden: String,
    pub note: String,
    pub todo: String,
    hidden_note: String,
    hidden_file: String,
}

pub struct Fixture {
    dir: tempfile::TempDir,
    handle: Handle,
    base: String,
    pub data: [AccountData; 2],
}

impl Fixture {
    pub async fn start() -> Self {
        let (dir, mut data) = setup();
        let handle = Server::start(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            data_dir: dir.path().into(),
            public_url: Some(ISSUER.into()),
            account_in_flight: 2,
            account_queue: 12,
            ..Config::default()
        })
        .await
        .unwrap();
        let base = format!("http://{}", handle.local_addr());
        for (index, entry) in data.iter_mut().enumerate() {
            entry
                .keys
                .push(oauth::token(&base, &handle, ACCOUNTS[index]));
        }
        Self {
            dir,
            handle,
            base,
            data,
        }
    }

    pub fn client<'a>(&self, account: usize, role: usize) -> Client<'a> {
        Client::new(&self.base, &self.data, account, role)
    }

    pub fn owner<'a>(&self, account: usize) -> Client<'a> {
        self.client(account, 0)
    }

    pub fn race(&self, calls: [(&str, Value); 2]) -> [Reply; 2] {
        let barrier = Barrier::new(2);
        thread::scope(|scope| {
            calls
                .map(|(tool, args)| {
                    let start = &barrier;
                    scope.spawn(move || {
                        let client = self.owner(0);
                        start.wait();
                        client.call(tool, args)
                    })
                })
                .map(|worker| worker.join().unwrap())
        })
    }

    pub fn one_winner(&self, calls: [(&str, Value); 2], error: &str) {
        let replies = self.race(calls);
        assert_eq!(
            replies.iter().filter(|reply| reply.is_ok()).count(),
            1,
            "{replies:?}"
        );
        assert_eq!(replies.into_iter().find_map(Result::err).unwrap(), error);
    }

    pub fn verify(&self) {
        verify(self.dir.path(), &self.base, &self.data);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle.shutdown();
    }
}

pub fn setup() -> (tempfile::TempDir, [AccountData; 2]) {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&target).unwrap();
    let dir = tempfile::tempdir_in(target).unwrap();
    let accounts = Accounts::open(dir.path()).unwrap();
    let data = ACCOUNTS.map(|name| seed_account(&accounts, name));
    (dir, data)
}

fn seed_account(accounts: &Accounts, name: &str) -> AccountData {
    use pensine_store::{ListQuery, NewFact, NewFile};
    let store = accounts.account_create(name).unwrap().store.clone();
    let hidden = format!("{name}-private");
    for space in ["shared", hidden.as_str()] {
        store.space_create(space, space, None).unwrap();
    }
    let todo = store
        .list(ListQuery {
            spaces: vec!["shared".into()],
            ..ListQuery::default()
        })
        .unwrap()
        .items
        .remove(0)
        .id;
    let note = |space: &str, suffix| {
        store
            .remember(NewFact {
                space: space.into(),
                text: format!("probe {name} {suffix}"),
                tags: vec!["fixture".into()],
                author: "fixture".into(),
                source: "fixture".into(),
                at: None,
                origin: None,
            })
            .unwrap()
            .fact
            .id
    };
    let archive = note("shared", "archived start");
    store.retire(&archive, "fixture").unwrap();
    let archived_head = store.restore(&archive).unwrap();
    store.retire(&archived_head.id, "fixture").unwrap();
    let hidden_file = store
        .file_put(NewFile {
            space: hidden.clone(),
            name: format!("{name}-private.bin"),
            mime: "application/octet-stream".into(),
            reader: name.as_bytes(),
            max_size: 10,
            note: None,
        })
        .unwrap()
        .id;
    let (_, owner) = accounts.connection_create_owner(name, "Owner").unwrap();
    let keys = std::iter::once(owner.as_str().to_owned())
        .chain([false, true].map(|read_only| {
            let (_, key) = accounts
                .connection_create(
                    name,
                    if read_only { "Reader" } else { "Writer" },
                    vec![Grant {
                        space: "shared".into(),
                        write: !read_only,
                    }],
                    read_only,
                )
                .unwrap();
            key.as_str().to_owned()
        }))
        .collect();
    AccountData {
        keys,
        note: note("shared", "start"),
        hidden_note: note(&hidden, "secret"),
        hidden_file,
        hidden,
        todo,
    }
}

pub fn verify(root: &Path, base: &str, data: &[AccountData; 2]) {
    for (index, account) in ACCOUNTS.iter().enumerate() {
        let client = Client::new(base, data, index, 0);
        invariant::check(&root.join("accounts").join(account), &client);
    }
}

pub async fn interrupted_files() {
    let (dir, data) = setup();
    let accounts = Accounts::open(dir.path()).unwrap();
    let store = accounts.account(ACCOUNTS[0]).unwrap().store;
    let live_path = store.file_get(&data[0].hidden_file).unwrap().1;
    let orphan = store
        .file_put(pensine_store::NewFile {
            space: "shared".into(),
            name: "interrupted.bin".into(),
            mime: "application/octet-stream".into(),
            reader: b"interrupted".as_slice(),
            max_size: 20,
            note: None,
        })
        .unwrap();
    let orphan_path = store.file_get(&orphan.id).unwrap().1;
    // Model a SIGKILL after a retirement commits and before unlink.
    let db = rusqlite::Connection::open(
        dir.path()
            .join("accounts")
            .join(ACCOUNTS[0])
            .join("store.db"),
    )
    .unwrap();
    db.execute(
        "UPDATE files SET status = 'retired' WHERE id = ?1",
        [&orphan.id],
    )
    .unwrap();
    let staged = live_path.parent().unwrap().join(".tmp-interrupted");
    std::fs::write(&staged, b"staging").unwrap();
    assert!(orphan_path.exists());
    let handle = Server::start(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: dir.path().into(),
        ..Config::default()
    })
    .await
    .unwrap();
    assert!(live_path.exists());
    // Startup removes the interrupted upload and keeps the orphan: only an explicit repair deletes it.
    assert!(orphan_path.exists());
    assert!(!staged.exists());
    assert!(!store.sweep_blobs(false).unwrap().orphans.is_empty());
    store.sweep_blobs(true).unwrap();
    assert!(!orphan_path.exists());
    verify(
        dir.path(),
        &format!("http://{}", handle.local_addr()),
        &data,
    );
    handle.shutdown();
}

pub fn upload(name: &str, bytes: &[u8]) -> Value {
    json!({"space":"shared","name":name,"base64":STANDARD.encode(bytes)})
}
