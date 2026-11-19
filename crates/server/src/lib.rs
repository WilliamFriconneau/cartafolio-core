//! memcp-server: MCP over HTTP, the OAuth endpoints and the admin page.

#[macro_use]
pub mod out;
pub mod admin;
pub mod app;
pub mod cli;
pub mod cli_crud;
pub mod cli_remote;
pub mod cli_table;
pub mod commands;
pub mod config;
pub mod crud;
pub mod discovery;
pub mod doctor;
pub mod explore;
pub mod folder_sync;
pub mod http;
pub mod intake;
pub mod join;
pub mod net;
pub mod oauth;
pub mod pair;
pub mod remote;
pub mod replica;
pub mod setup;
pub mod stdio;
pub mod sync_http;
pub mod tar;
pub(crate) mod view;
