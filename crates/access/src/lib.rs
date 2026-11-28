#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout
)]

#[cfg(test)]
mod key_tests;

mod account;
mod alias;
mod error;
mod grant;
mod key;

pub use account::{Account, Accounts};
pub use alias::validate_alias;
pub use error::AccessError;
pub use grant::{Grant, Principal};
pub use key::{Connection, SecretKey};
