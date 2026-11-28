#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod config;
mod error;
mod http;
mod oauth;

pub use config::Config;
pub use error::ServerError;
pub use http::{Authenticator, Handle, KeysAuthenticator, Server};
