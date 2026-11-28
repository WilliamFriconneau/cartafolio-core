#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::todo,
    clippy::dbg_macro,
    clippy::print_stdout
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod admin;
mod admin_html;
mod admission;
mod config;
mod error;
mod html;
mod http;
mod mcp_http;
mod oauth;
mod texts;

pub use config::Config;
pub use error::ServerError;
pub use http::{serve_blocking, Authenticator, Handle, KeysAuthenticator, Server};
