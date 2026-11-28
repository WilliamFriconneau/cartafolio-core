#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use serde_json::json;
use support::{Fixture, CONFLICT, MISSING, PERMANENT};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_clients_keep_every_invariant() {
    support::workload::run().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replayed_origins_from_two_clients_create_one_note() {
    let env = Fixture::start().await;
    let args =
        json!({"space":"shared","notes":[{"text":"probe replay","origin":["queue","race"]}]});
    let replies = env.race([("remember_many", args.clone()), ("remember_many", args)]);
    let [a, b] = replies.map(|reply| reply.unwrap());
    assert_eq!(a[0]["fact"]["id"], b[0]["fact"]["id"]);
    let mut outcomes = [
        a[0]["outcome"].as_str().unwrap(),
        b[0]["outcome"].as_str().unwrap(),
    ];
    outcomes.sort_unstable();
    assert_eq!(outcomes, ["created", "replayed"]);
    env.verify();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_one_of_two_racing_supersedes_wins() {
    let env = Fixture::start().await;
    let id = &env.data[0].note;
    let args = |text| json!({"id":id,"text":text,"reason":"edit"});
    env.one_winner(
        [
            ("supersede", args("probe first")),
            ("supersede", args("probe second")),
        ],
        CONFLICT,
    );
    env.verify();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn racing_moves_and_retires_never_leave_two_heads() {
    let env = Fixture::start().await;
    let id = &env.data[0].note;
    env.one_winner(
        [
            ("move", json!({"ids":[id],"to_space":env.data[0].hidden})),
            ("retire", json!({"id":id,"reason":"race"})),
        ],
        CONFLICT,
    );
    env.verify();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identical_file_uploads_share_one_blob_and_survive_a_retire_race() {
    let env = Fixture::start().await;
    let [a, b] = env
        .race([
            ("file_put", support::upload("first", b"same")),
            ("file_put", support::upload("second", b"same")),
        ])
        .map(|reply| reply.unwrap());
    assert_ne!(a["id"], b["id"]);
    assert_eq!(a["sha256"], b["sha256"]);
    let client = env.owner(0);
    for id in [&a["id"], &b["id"]] {
        assert_eq!(
            client.call("file_get", json!({"id":id})).unwrap()["base64"],
            "c2FtZQ=="
        );
    }
    client
        .call(
            "file_retire",
            json!({"id":b["id"],"reason":"leave one reference"}),
        )
        .unwrap();
    let [retired, uploaded] = env
        .race([
            ("file_retire", json!({"id":a["id"],"reason":"race"})),
            ("file_put", support::upload("third", b"same")),
        ])
        .map(|reply| reply.unwrap());
    assert_eq!(retired, json!({}));
    assert_eq!(
        client
            .call("file_get", json!({"id":uploaded["id"]}))
            .unwrap()["base64"],
        "c2FtZQ=="
    );
    env.verify();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn illegal_operations_are_refused_under_load() {
    let env = Fixture::start().await;
    let owner = env.owner(0);
    let data = &env.data[0];
    let edit = || json!({"id":data.note,"text":"probe successor","reason":"edit"});
    owner.call("supersede", edit()).unwrap();
    let start = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let start = &start;
        let env = &env;
        let edit = &edit;
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                scope.spawn(move || {
                    let client = env.client(0, usize::from(worker == 2));
                    start.wait();
                    for step in 0..20 {
                        let (tool, args, error) = match worker {
                            0 => (
                                "retire",
                                json!({"id":data.todo,"reason":"illegal"}),
                                Some(PERMANENT),
                            ),
                            1 => ("supersede", edit(), Some(CONFLICT)),
                            2 => (
                                "remember",
                                json!({"space":data.hidden,"text":"illegal"}),
                                Some(MISSING),
                            ),
                            _ => (
                                "remember",
                                json!({"space":"shared","text":format!("probe load{step}")}),
                                None,
                            ),
                        };
                        let result = client.call(tool, args);
                        match error {
                            Some(error) => assert_eq!(result.unwrap_err(), error),
                            None => {
                                result.unwrap();
                            }
                        }
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    env.verify();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_recovers_blobs_left_by_interrupted_file_operations() {
    support::interrupted_files().await;
}

#[cfg(unix)]
#[test]
#[ignore = "kills real server processes; run with PENSINE_KILL=1 and --ignored"]
fn killing_the_server_never_breaks_an_invariant() {
    assert_eq!(std::env::var("PENSINE_KILL").as_deref(), Ok("1"));
    support::crash::run();
}
