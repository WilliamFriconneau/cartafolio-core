use super::{upload, AccountData, Client, Fixture, Reply, ACCOUNTS, CONFLICT, MISSING, PERMANENT};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::{json, Value};
use std::sync::{Barrier, Mutex};
use std::thread;
use std::time::Instant;

const STEPS: usize = 300;

pub fn seed() -> u64 {
    std::env::var("PENSINE_SEED").map_or_else(|_| rand::random(), |value| value.parse().unwrap())
}

pub async fn run() {
    let seed = seed();
    let workers = if std::env::var("PENSINE_STRESS").as_deref() == Ok("1") {
        32
    } else {
        16
    };
    eprintln!("concurrent workload seed={seed} clients={workers} steps={STEPS}");
    let env = Fixture::start().await;
    let audit = super::audit::Audit::default();
    let state = [Mutex::new(State::default()), Mutex::new(State::default())];
    let barrier = Barrier::new(workers);
    let start = Instant::now();
    thread::scope(|scope| {
        let threads: Vec<_> = (0..workers)
            .map(|worker| {
                let account = worker / 4 % 2;
                let role = worker % 4;
                let client = env.client(account, role).audited(&audit);
                let state = &state[account];
                let barrier = &barrier;
                let data = &env.data;
                scope.spawn(move || {
                    let mut run = Worker::new(client, data, state, seed, worker, role, 0);
                    barrier.wait();
                    for step in 0..STEPS {
                        run.step(step).unwrap_or_else(|error| {
                            panic!("seed={seed} worker={worker} step={step}: {error}")
                        });
                    }
                })
            })
            .collect();
        for worker in threads {
            worker.join().unwrap();
        }
    });
    report("concurrent", workers * STEPS, start);
    for (account, state) in state.iter().enumerate() {
        let expected = state.lock().unwrap().replays.clone();
        let reader = env.owner(account);
        for (origin, id, outcome) in &expected {
            let matches: Vec<_> = expected.iter().filter(|entry| entry.0 == *origin).collect();
            assert_eq!(matches.len(), 2);
            assert_eq!(
                matches.iter().filter(|entry| entry.2 == "created").count(),
                1
            );
            assert!(matches.iter().all(|entry| entry.1 == *id));
            let reply = reader.call("read", json!({"id":id})).unwrap();
            assert_eq!(reply["origin"], json!(["queue", origin]));
            assert!(matches!(outcome.as_str(), "created" | "replayed"));
        }
        assert_eq!(expected.len(), workers / 4);
    }
    audit.verify(env.dir.path());
    env.verify();
}

pub fn report(label: &str, operations: usize, start: Instant) {
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!(
        "{label}: {operations} logical operations in {elapsed:.3}s ({:.0} ops/s)",
        operations as f64 / elapsed
    );
}

#[derive(Clone, PartialEq)]
struct Head {
    id: String,
    hidden: bool,
    retired: bool,
}

#[derive(Default)]
pub struct State {
    head: Option<Head>,
    file: Option<String>,
    replays: Vec<(String, Value, String)>,
}

#[cfg(unix)]
impl State {
    pub fn with_head(id: String) -> Self {
        Self {
            head: Some(Head {
                id,
                hidden: false,
                retired: false,
            }),
            ..Self::default()
        }
    }
}

pub struct Worker<'a> {
    client: Client<'a>,
    data: &'a [AccountData; 2],
    state: &'a Mutex<State>,
    rng: StdRng,
    worker: usize,
    account: usize,
    role: usize,
    round: usize,
    initial: String,
}

impl<'a> Worker<'a> {
    pub fn new(
        client: Client<'a>,
        data: &'a [AccountData; 2],
        state: &'a Mutex<State>,
        seed: u64,
        worker: usize,
        role: usize,
        round: usize,
    ) -> Self {
        let account = worker / 4 % 2;
        let initial = state
            .lock()
            .unwrap()
            .head
            .as_ref()
            .map_or_else(|| data[account].note.clone(), |head| head.id.clone());
        Self {
            client,
            data,
            state,
            rng: StdRng::seed_from_u64(seed.wrapping_add(worker as u64)),
            worker,
            account,
            role,
            round,
            initial,
        }
    }

    pub fn step(&mut self, step: usize) -> Result<(), String> {
        let home = &self.data[self.account];
        let other = &self.data[1 - self.account];
        let serial = format!(
            "{}-r{}w{}s{step}",
            ACCOUNTS[self.account], self.round, self.worker
        );
        let text = format!("probe {} {serial}", ACCOUNTS[self.account]);
        let (head, file) = {
            let state = self.state.lock().unwrap();
            (
                state.head.clone().unwrap_or_else(|| Head {
                    id: self.initial.clone(),
                    hidden: false,
                    retired: false,
                }),
                state.file.clone(),
            )
        };
        if step == 0 && self.role < 2 {
            let origin = format!("r{}pair{}", self.round, self.worker / 4);
            let response = self.client.request("remember_many", json!({"space":"shared","notes":[
                {"text":format!("probe {} {origin}", ACCOUNTS[self.account]),"origin":["queue",origin]}
            ]}))?.unwrap();
            self.state.lock().unwrap().replays.push((
                origin,
                response[0]["fact"]["id"].clone(),
                response[0]["outcome"].as_str().unwrap().into(),
            ));
            return Ok(());
        }
        let choice = self
            .rng
            .random_range(0..if self.role < 2 { 23 } else { 10 });
        let (tool, args, expected) = match choice {
            0 => ("recall", json!({"query":"probe","limit":10}), None),
            1 => ("list", json!({"limit":10}), None),
            2 => ("spaces", json!({}), None),
            3 => ("access", json!({}), None),
            4 => ("read", json!({"id":home.note}), None),
            5 => ("history", json!({"id":home.note}), None),
            6 => ("read", json!({"id":other.note}), Some(MISSING)),
            7 => ("file_get", json!({"id":other.hidden_file}), Some(MISSING)),
            8 if self.role > 1 => (
                "remember",
                json!({"space":"shared","text":text}),
                Some("Write access required"),
            ),
            8 => (
                "retire",
                json!({"id":home.todo,"reason":"illegal"}),
                Some(PERMANENT),
            ),
            9 if self.role != 0 => (
                "remember",
                json!({"space":home.hidden,"text":text}),
                Some(MISSING),
            ),
            9 => (
                "remember",
                json!({"space":other.hidden,"text":text}),
                Some(MISSING),
            ),
            10 => (
                "remember",
                json!({"space":"shared","text":text,"tags":["one"]}),
                None,
            ),
            11 => (
                "remember_many",
                json!({"space":"shared","notes":[{"text":text},{"text":format!("{text} extra") }]}),
                None,
            ),
            12..=17 if self.role == 0 || !head.hidden => return self.mutate(head, choice, text),
            18 | 19 => {
                let bytes = if choice == 18 {
                    b"shared bytes".as_slice()
                } else {
                    serial.as_bytes()
                };
                let result = self
                    .client
                    .request("file_put", upload(&format!("{}.bin", serial), bytes))?
                    .unwrap();
                assert_eq!(result["size"], bytes.len());
                self.state.lock().unwrap().file = Some(id(&result));
                return Ok(());
            }
            20 if file.is_some() => ("file_retire", json!({"id":file,"reason":"workload"}), None),
            21 if self.role == 0 => (
                "space_create",
                json!({"name":serial,"label":"Workload"}),
                None,
            ),
            22 if head.id != self.initial => (
                "supersede",
                json!({"id":self.initial,"text":text,"reason":"illegal"}),
                Some(CONFLICT),
            ),
            _ => ("file_list", json!({"limit":10}), None),
        };
        let reply = self.client.request(tool, args)?;
        if tool == "file_retire" && reply == Err(MISSING.into()) {
            // Another client retired the same active file first.
            return Ok(());
        }
        check(tool, reply, expected);
        Ok(())
    }

    fn mutate(&self, head: Head, choice: usize, text: String) -> Result<(), String> {
        let (tool, args) = if head.retired {
            ("restore", json!({"id":head.id}))
        } else {
            match choice {
                12 => (
                    "supersede",
                    json!({"id":head.id,"text":text,"reason":"workload"}),
                ),
                13 => ("tags_set", json!({"id":head.id,"tags":["mixed"]})),
                14 | 15 => ("pin", json!({"id":head.id,"pinned":choice == 14})),
                16 => ("retire", json!({"id":head.id,"reason":"workload"})),
                _ if self.role == 0 => (
                    "move",
                    json!({"ids":[head.id],"to_space":if head.hidden {"shared"} else {&self.data[self.account].hidden}}),
                ),
                _ => ("pin", json!({"id":head.id,"pinned":false})),
            }
        };
        match self.client.request(tool, args)? {
            Err(error) => assert_eq!(error, CONFLICT, "{tool}"),
            Ok(result) => {
                let next = match tool {
                    "pin" => {
                        assert_eq!(result["id"], head.id);
                        assert_eq!(result["pinned"], args_pin(choice, self.role));
                        return Ok(());
                    }
                    "retire" => Head {
                        retired: true,
                        ..head.clone()
                    },
                    _ => {
                        let fact = if tool == "move" { &result[0] } else { &result };
                        assert_eq!(fact["replaces"], head.id);
                        Head {
                            id: id(fact),
                            hidden: fact["space"] != "shared",
                            retired: false,
                        }
                    }
                };
                let mut state = self.state.lock().unwrap();
                if state.head.as_ref().is_none_or(|current| *current == head) {
                    state.head = Some(next);
                }
            }
        }
        Ok(())
    }
}

fn args_pin(choice: usize, role: usize) -> bool {
    debug_assert!(choice == 14 || choice == 15 || role != 0);
    choice == 14
}

fn id(fact: &Value) -> String {
    fact["id"].as_str().unwrap().into()
}

fn check(tool: &str, reply: Reply, expected: Option<&str>) {
    match expected {
        Some(expected) => assert_eq!(reply.unwrap_err(), expected, "{tool}"),
        None => {
            assert!(reply.is_ok(), "{tool}: {reply:?}");
        }
    }
}
