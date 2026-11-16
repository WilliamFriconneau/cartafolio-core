//! Where the key may go. HTTPS anywhere; plain HTTP only toward the person's own network
//! (this machine, a private or tailnet address, a `.local` or `.lan` name): the key never crosses the
//! internet in clear.

use ureq::http::Uri;

/// Whether the key may be sent to `text`: the one rule that keeps it off the internet in clear.
pub fn allowed_url(text: &str) -> bool {
    let Ok(uri) = text.parse::<Uri>() else {
        return false;
    };
    let Some(host) = uri.host().map(str::to_ascii_lowercase) else {
        return false;
    };
    match uri.scheme_str() {
        Some("https") => true,
        Some("http") => own_network(&host),
        _ => false,
    }
}

fn own_network(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".local") || host.ends_with(".lan") {
        return true;
    }
    if host == "[::1]" {
        return true;
    }
    // Only a strict dotted quad: `010.0.0.1` would pass a loose parse as 10.0.0.1 while the system's resolver
    // reads it in octal as 8.0.0.1, a public address. A host made of digits and dots that is not a strict
    // quad is refused.
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            let o = ip.octets();
            ip.is_loopback()
                || o[0] == 10
                || (o[0] == 192 && o[1] == 168)
                // The tailnet (100.64.0.0/10, the carrier-grade range Tailscale uses): the person's own devices.
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 172 && (16..=31).contains(&o[1]))
        }
        Err(_) => false,
    }
}
