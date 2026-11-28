#![allow(clippy::unwrap_used, clippy::expect_used)]

use pensine_server::Config;

#[test]
fn plain_http_exposure_requires_the_option() {
    let base = "listen = '0.0.0.0:8791'\nallow_remote = true\n";
    let denied = Config::from_toml(&format!("{base}allow_plain_http = false")).unwrap();
    assert!(denied.listen.ip().is_loopback());
    let allowed = Config::from_toml(&format!("{base}allow_plain_http = true")).unwrap();
    assert!(allowed.listen.ip().is_unspecified());
}
