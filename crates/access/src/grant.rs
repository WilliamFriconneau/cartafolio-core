use crate::{AccessError, Account, Connection};
use pensine_store::StoreError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub space: String,
    pub write: bool,
}

#[derive(Clone)]
pub struct Principal {
    pub account: String,
    pub connection: String,
    pub grants: Vec<Grant>,
    read_only: bool,
    scope: Account,
}

impl Principal {
    pub fn new(account: Account, connection: Connection) -> Result<Self, AccessError> {
        if account.id != connection.account {
            return Err(AccessError::NotFound);
        }
        Ok(Self {
            account: account.id.clone(),
            connection: connection.id,
            grants: connection.grants,
            read_only: connection.read_only,
            scope: account,
        })
    }

    pub fn may_write(&self) -> bool {
        !self.read_only && self.grants.iter().any(|grant| grant.write)
    }

    pub fn can_write_root(&self) -> bool {
        !self.read_only
            && self
                .grants
                .iter()
                .any(|grant| grant.space == "*" && grant.write)
    }

    pub fn can_read(&self, space: &str) -> bool {
        self.space_permissions(space).is_ok()
    }

    pub fn can_write(&self, space: &str) -> bool {
        self.space_permissions(space).unwrap_or(false)
    }

    /// Returns whether the readable space is writable.
    pub fn space_permissions(&self, space: &str) -> Result<bool, StoreError> {
        let chain = self.scope.store.space_chain(space)?;
        let mut grants = self
            .grants
            .iter()
            .filter(|grant| grant.space == "*" || chain.iter().any(|name| name == &grant.space))
            .peekable();
        if grants.peek().is_none() {
            return Err(StoreError::NotFound);
        }
        Ok(!self.read_only && grants.any(|grant| grant.write))
    }

    pub fn belongs_to(&self, account: &Account) -> bool {
        self.account == account.id && std::sync::Arc::ptr_eq(&self.scope.store, &account.store)
    }
}
