//! `memcp-bridge connect ADDRESS`: carries standard input and output to a gateway served over HTTP on this machine
//! (`--listen`), the way `connect SOCKET` does for a Unix socket. It is how an assistant that runs a command reaches
//! the bridge on Windows, which has no Unix socket, and works the same everywhere.
//!
//! One JSON-RPC message per line goes in; the answer, when there is one, comes out as one line. Only a loopback
//! address is accepted: the bridge's gateway is never served to the network.

use std::io::{self, BufRead, Write};
use std::time::Duration;

/// Whether `target` names an HTTP gateway (`host:port` or `http://host:port`) rather than a socket path.
pub fn is_address(target: &str) -> bool {
    let t = target.strip_prefix("http://").unwrap_or(target);
    !t.contains('/')
        && t.rsplit_once(':')
            .is_some_and(|(h, p)| !h.is_empty() && p.parse::<u16>().is_ok())
}

fn loopback(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    h == "localhost" || h == "127.0.0.1" || h == "::1"
}

/// Relays lines from `input` to `POST http://ADDRESS/mcp` and writes each answer to `output`.
pub fn relay<R: BufRead, W: Write>(target: &str, input: R, mut output: W) -> io::Result<()> {
    let t = target.strip_prefix("http://").unwrap_or(target);
    let host = t.rsplit_once(':').map_or(t, |(h, _)| h);
    if !is_address(target) || !loopback(host) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "connect: the gateway must be on this machine (127.0.0.1:PORT)",
        ));
    }
    let url = format!("http://{t}/mcp");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut resp = agent
            .post(&url)
            .header("Content-Type", "application/json")
            .send(line.as_bytes())
            .map_err(|e| io::Error::other(format!("the gateway cannot be reached: {e}")))?;
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| io::Error::other(format!("the gateway answered badly: {e}")))?;
        if status == 202 || status == 204 || body.trim().is_empty() {
            continue;
        }
        // One line each, whatever the gateway's formatting.
        writeln!(output, "{}", body.trim().replace(['\r', '\n'], " "))?;
        output.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{is_address, relay};
    use std::io::Cursor;

    #[test]
    fn an_address_is_told_from_a_path() {
        assert!(is_address("127.0.0.1:8792"));
        assert!(is_address("http://localhost:8792"));
        assert!(!is_address("/run/user/1000/memcp.sock"));
        assert!(!is_address("C:\\sockets\\memcp"));
        assert!(!is_address("memcp.sock"));
    }

    #[test]
    fn lines_go_to_the_gateway_and_its_answers_come_back_one_per_line() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("listen");
        let addr = server.server_addr().to_string();
        let h = std::thread::spawn(move || {
            let mut seen = vec![];
            for _ in 0..2 {
                let mut rq = server.recv().expect("request");
                let mut body = String::new();
                rq.as_reader().read_to_string(&mut body).expect("body");
                seen.push((rq.url().to_string(), body.clone()));
                if body.contains("notifications/") {
                    rq.respond(tiny_http::Response::empty(202))
                        .expect("respond");
                } else {
                    rq.respond(tiny_http::Response::from_string(
                        "{\"jsonrpc\":\"2.0\",\n\"id\":1,\"result\":{}}",
                    ))
                    .expect("respond");
                }
            }
            seen
        });
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        let mut out = Vec::new();
        relay(&addr, Cursor::new(input), &mut out).expect("relayed");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "{\"jsonrpc\":\"2.0\", \"id\":1,\"result\":{}}\n",
            "one answer; the notification has none"
        );
        let seen = h.join().expect("server");
        assert!(seen.iter().all(|(u, _)| u == "/mcp"));
    }

    #[test]
    fn a_gateway_that_is_not_on_this_machine_is_refused() {
        let err = relay("192.168.1.20:8792", Cursor::new("{}\n"), Vec::new()).expect_err("refused");
        assert!(err.to_string().contains("this machine"), "{err}");
    }
}
