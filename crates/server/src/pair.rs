//! `POST /pair`: a capture program exchanges the code the person's page showed for a connection of its own.
//!
//! No key is needed (the device has none yet), so the route is guarded like the key routes: a few
//! tries a minute per address, and an address that keeps failing is held off for a while.

use memcp_core::{pairing, util};
use serde_json::{Value, json};

use crate::app::App;
use crate::http::{Request, Response};

/// Tries a minute from one address.
const PER_MIN: i64 = 10;

fn refused(status: u16, message: &str) -> Response {
    Response::json(status, &json!({ "error": message }))
}

/// Whether this installation holds the code of a pairing request (the manager finds the person this way).
pub fn holds(app: &App, body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(code) = v.get("code").and_then(Value::as_str) else {
        return false;
    };
    app.brain
        .open_control()
        .is_ok_and(|ctl| pairing::holds(&ctl, code, util::now()))
}

pub fn exchange(app: &App, req: &Request) -> Response {
    let now = util::now();
    let who = req.ip.clone().unwrap_or_else(|| "?".into());
    if app.blocked(now, &who) || !app.rate_ok(now, &format!("pair:{who}"), PER_MIN) {
        return refused(429, "too many tries, wait a few minutes").with("Retry-After", "60");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return refused(400, "JSON expected: {\"code\": ..., \"device\": ...}");
    };
    let code = body.get("code").and_then(Value::as_str).unwrap_or("");
    let device = body.get("device").and_then(Value::as_str).unwrap_or("");
    let Ok(ctl) = app.brain.open_control() else {
        return refused(500, "storage failed");
    };
    match pairing::redeem(&ctl, code, device, now) {
        Ok(p) => {
            memcp_core::access::auth_event(
                &ctl,
                Some(&p.agent),
                req.ip.as_deref(),
                "paired",
                device,
            );
            Response::json(
                200,
                &json!({
                    "agent": p.agent,
                    "key": p.token,
                    "mcp_url": format!("{}/mcp/{}", app.public_url, p.agent),
                    "vaults": p.spaces,
                }),
            )
        }
        Err(e) => {
            app.fail(now, &who);
            refused(400, &e.to_string())
        }
    }
}
