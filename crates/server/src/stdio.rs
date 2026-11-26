//! The MCP server over standard input and output, for an assistant that runs a local
//! command (a desktop assistant, a coding agent): no port, no address, nothing on the
//! network. One JSON message per line in, one per line out; nothing else is ever written to
//! standard output, so a program reading it sees only protocol.
//!
//! It is the same server as over HTTP (`App::dispatch`), with the rights of a key made by
//! `open` or `connect`. The key is checked again on every request, so a revoked key stops
//! the server at once.

use std::io::{BufRead, Write};

use pensine_core::access;
use pensine_core::util;
use serde_json::{Value, json};

use crate::app::{App, rpc_err};

fn send<W: Write>(out: &mut W, v: &Value) -> Result<(), String> {
    let line = serde_json::to_string(v).map_err(|e| e.to_string())?;
    out.write_all(line.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .and_then(|()| out.flush())
        .map_err(|e| e.to_string())
}

/// Serves requests from `input` until it ends. `Err` if the key is refused or revoked: the
/// reason, for the caller to say on standard error.
pub fn run<R: BufRead, W: Write>(
    app: &App,
    token: &str,
    input: R,
    mut out: W,
) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                send(
                    &mut out,
                    &rpc_err(&Value::Null, -32700, "Parse error", None),
                )?;
                continue;
            }
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str);
        let (Some(method), true) = (method, msg.get("jsonrpc") == Some(&json!("2.0"))) else {
            // An answer from the client to us (we send no request), or something that is not
            // a request: said only when it had an id to answer to.
            if msg.get("result").is_none() && msg.get("error").is_none() {
                send(
                    &mut out,
                    &rpc_err(&id.unwrap_or(Value::Null), -32600, "Invalid Request", None),
                )?;
            }
            continue;
        };
        // A notification gets no answer.
        let Some(id) = id else { continue };
        let now = util::now();
        let ctl = app.brain.open_control().map_err(|e| e.to_string())?;
        let Some(mut caller) = access::authenticate(&ctl, token, now).ok().flatten() else {
            access::auth_event(&ctl, None, None, "mcp_refused", "standard input");
            send(
                &mut out,
                &rpc_err(&id, -32001, "The key is refused or revoked", None),
            )?;
            return Err("the key is refused or revoked".into());
        };
        // The standard input of a program started on this machine.
        caller.local = true;
        let _ = access::touch(&ctl, &caller, None, now);
        if !app.rate_ok(now, &caller.id, caller.rate_per_min) {
            send(&mut out, &rpc_err(&id, -32000, "Rate limit exceeded", None))?;
            continue;
        }
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let (_, body) = app.dispatch(&caller, &id, method, &params, now);
        send(&mut out, &body)?;
    }
    Ok(())
}
