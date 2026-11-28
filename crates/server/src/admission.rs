use crate::{Config, ServerError};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{timeout_at, Instant};

pub(crate) struct Admission {
    global: Arc<Budget>,
    account_limits: (usize, usize),
    accounts: Mutex<HashMap<String, Weak<Budget>>>,
}

struct Budget {
    running: Arc<Semaphore>,
    queue: Arc<Semaphore>,
}

pub(crate) struct Admitted {
    _running: [OwnedSemaphorePermit; 2],
    _account: Arc<Budget>,
}

impl Admission {
    pub fn new(config: &Config) -> Result<Self, ServerError> {
        for (running, queue) in [
            (config.account_in_flight, config.account_queue),
            (config.global_in_flight, config.global_queue),
        ] {
            if running == 0 || running.saturating_add(queue) > Semaphore::MAX_PERMITS {
                return Err(ServerError::Invalid("Invalid MCP admission limits".into()));
            }
        }
        Ok(Self {
            global: Arc::new(Budget::new(config.global_in_flight, config.global_queue)),
            account_limits: (config.account_in_flight, config.account_queue),
            accounts: Mutex::default(),
        })
    }

    pub async fn enter(&self, id: &str, deadline: Instant) -> Option<Admitted> {
        let account = self.account_budget(id)?;
        let running = match account.try_run_with(&self.global) {
            Some(running) => running,
            None => {
                let _account_wait = account.queue.try_acquire().ok()?;
                let _global_wait = self.global.queue.try_acquire().ok()?;
                timeout_at(deadline, async {
                    let account_run = account.running.clone().acquire_owned().await.ok()?;
                    let global_run = self.global.running.clone().acquire_owned().await.ok()?;
                    Some([account_run, global_run])
                })
                .await
                .ok()??
            }
        };
        Some(Admitted {
            _account: account,
            _running: running,
        })
    }

    fn account_budget(&self, id: &str) -> Option<Arc<Budget>> {
        let mut budgets = self.accounts.lock().ok()?;
        if let Some(account) = budgets.get(id).and_then(Weak::upgrade) {
            return Some(account);
        }
        budgets.retain(|_, budget| budget.strong_count() != 0);
        let account = Arc::new(Budget::new(self.account_limits.0, self.account_limits.1));
        budgets.insert(id.to_owned(), Arc::downgrade(&account));
        Some(account)
    }
}

impl Budget {
    fn try_run_with(&self, global: &Self) -> Option<[OwnedSemaphorePermit; 2]> {
        let account_run = self.running.clone().try_acquire_owned().ok()?;
        let global_run = global.running.clone().try_acquire_owned().ok()?;
        Some([account_run, global_run])
    }

    fn new(running: usize, queue: usize) -> Self {
        Self {
            running: Arc::new(Semaphore::new(running)),
            queue: Arc::new(Semaphore::new(queue)),
        }
    }
}
