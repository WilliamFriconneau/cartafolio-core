use crate::{AccessError, Account};
use std::sync::{Condvar, Mutex};

#[derive(Default)]
pub(crate) struct Operations {
    held: Mutex<bool>,
    available: Condvar,
}

pub struct AccountOperations {
    account: Account,
}

impl AccountOperations {
    pub fn account(&self) -> &Account {
        &self.account
    }
}

impl Account {
    /// Serializes permission checks and the operations they authorize.
    pub fn lock_operations(&self) -> Result<AccountOperations, AccessError> {
        let operations = &self.operations;
        let held = operations.held.lock().map_err(|_| AccessError::Conflict)?;
        let mut held = operations
            .available
            .wait_while(held, |held| *held)
            .map_err(|_| AccessError::Conflict)?;
        *held = true;
        Ok(AccountOperations {
            account: self.clone(),
        })
    }

    pub fn try_lock_operations(&self) -> Result<Option<AccountOperations>, AccessError> {
        let mut held = match self.operations.held.try_lock() {
            Ok(held) => held,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err(AccessError::Conflict),
        };
        if *held {
            return Ok(None);
        }
        *held = true;
        Ok(Some(AccountOperations {
            account: self.clone(),
        }))
    }
}

impl Drop for AccountOperations {
    fn drop(&mut self) {
        let operations = &self.account.operations;
        if let Ok(mut held) = operations.held.lock() {
            *held = false;
            operations.available.notify_one();
        }
    }
}
