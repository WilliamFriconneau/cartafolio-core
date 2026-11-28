use super::workload::{report, seed, State, Worker};
use super::{agent, audit::Audit, setup, verify, Client, ACCOUNTS, ISSUER};
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::json;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Barrier, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

pub fn run() {
    let seed = seed();
    eprintln!("SIGKILL workload seed={seed} clients=8 rounds=5");
    let mut rng = StdRng::seed_from_u64(seed);
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = workspace.join("target");
    let status = Command::new(env!("CARGO"))
        .current_dir(&workspace)
        .args([
            "build",
            "--locked",
            "-p",
            "pensine",
            "--bin",
            "pensine",
            "--target-dir",
        ])
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success(), "could not build pensine");
    let binary = target.join("debug/pensine");
    let (dir, data) = setup();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");
    let base = format!("http://{addr}");
    let config = dir.path().join("server.toml");
    std::fs::write(
        &config,
        format!("listen = \"{addr}\"\npublic_url = \"{ISSUER}\"\n"),
    )
    .unwrap();
    let start = || Process::start(&binary, dir.path(), &config, &base);
    let mut process = start();
    for round in 1..=5 {
        let stop = AtomicBool::new(false);
        let ready = Barrier::new(9);
        let in_flight = AtomicUsize::new(0);
        let audit = Audit::default();
        let state = std::array::from_fn::<_, 2, _>(|account| {
            let client = Client::new(&base, &data, account, 0);
            let text = format!("probe {} crashround{round}", ACCOUNTS[account]);
            let fact = client
                .call("remember", json!({"space":"shared","text":text}))
                .unwrap();
            Mutex::new(State::with_head(
                fact["fact"]["id"].as_str().unwrap().into(),
            ))
        });
        let delay = rng.random_range(50..=150);
        eprintln!("SIGKILL round={round} delay={delay}ms");
        let started = Instant::now();
        let completed = thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|worker| {
                    let account = worker / 4;
                    let role = (worker % 4).min(2);
                    let client = Client::new(&base, &data, account, role).audited(&audit);
                    let data = &data;
                    let state = &state[account];
                    let ready = &ready;
                    let stop = &stop;
                    let in_flight = &in_flight;
                    scope.spawn(move || {
                        let mut run = Worker::new(
                            client,
                            data,
                            state,
                            seed ^ round as u64,
                            worker,
                            role,
                            round,
                        );
                        ready.wait();
                        let mut completed = 0;
                        while !stop.load(Ordering::Acquire) {
                            in_flight.fetch_add(1, Ordering::SeqCst);
                            let reply = run.step(completed);
                            in_flight.fetch_sub(1, Ordering::SeqCst);
                            match reply {
                                Ok(()) => completed += 1,
                                Err(_) if stop.load(Ordering::Acquire) => break,
                                Err(error) => panic!("transport failed before SIGKILL: {error}"),
                            }
                        }
                        completed
                    })
                })
                .collect();
            ready.wait();
            thread::sleep(Duration::from_millis(delay));
            let deadline = Instant::now() + Duration::from_secs(2);
            while in_flight.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
                thread::yield_now();
            }
            let was_serving = in_flight.load(Ordering::SeqCst) > 0;
            stop.store(true, Ordering::Release);
            process.kill();
            assert!(was_serving, "no in-flight workload at SIGKILL");
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .sum()
        });
        assert!(completed > 0, "SIGKILL preceded every completed operation");
        report("SIGKILL round", completed, started);
        process = start();
        // The explicit repair an operator runs (`pensine check --fix`): orphans left by the crash are removed.
        for account in super::ACCOUNTS {
            let path = dir.path().join("accounts").join(account);
            pensine_store::Store::open(&path)
                .unwrap()
                .sweep_blobs(true)
                .unwrap();
        }
        audit.verify(dir.path());
        verify(dir.path(), &base, &data);
    }
}

struct Process(Child);

impl Process {
    fn start(binary: &Path, data: &Path, config: &Path, base: &str) -> Self {
        let mut process = Self(
            Command::new(binary)
                .arg("--data")
                .arg(data)
                .args(["serve", "--config"])
                .arg(config)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let agent = agent();
        let started = Instant::now();
        loop {
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "server exited during startup"
            );
            if agent.get(&format!("{base}/healthz")).call().is_ok() {
                return process;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "server startup timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill(&mut self) {
        use std::os::unix::process::ExitStatusExt;
        self.0.kill().unwrap();
        assert_eq!(self.0.wait().unwrap().signal(), Some(9));
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
