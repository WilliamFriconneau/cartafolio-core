use super::{agent, ISSUER};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use pensine_server::Handle;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

pub fn token(base: &str, handle: &Handle, account: &str) -> String {
    let agent = agent();
    let callback = "https://client.example.test/callback";
    let verifier = "abcdefghijklmnopqrstuvwxyz0123456789-._~abcdef";
    let register = agent
        .post(&format!("{base}/register"))
        .set("Content-Type", "application/json")
        .send_string(&json!({"redirect_uris":[callback]}).to_string())
        .unwrap();
    assert_eq!(register.status(), 201);
    let registration: Value = serde_json::from_str(&register.into_string().unwrap()).unwrap();
    let client = registration["client_id"].as_str().unwrap();
    let resource = format!("{ISSUER}/mcp/{account}");
    let authorize = format!("{base}/authorize");
    let html = agent
        .post(&authorize)
        .send_form(&[
            ("response_type", "code"),
            ("client_id", client),
            ("redirect_uri", callback),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)),
            ),
            ("code_challenge_method", "S256"),
            ("resource", &resource),
            ("invite", &handle.oauth_invitation(account).unwrap()),
        ])
        .unwrap()
        .into_string()
        .unwrap();
    let nonce = html
        .split_once("name=\"nonce\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap();
    let consent = agent
        .post(&authorize)
        .send_form(&[
            ("nonce", nonce),
            ("decision", "allow"),
            ("v_shared", "1"),
            ("readonly", "1"),
        ])
        .unwrap();
    assert_eq!(consent.status(), 302);
    let redirect = Url::parse(consent.header("Location").unwrap()).unwrap();
    let (_, code) = redirect
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap();
    let response = agent
        .post(&format!("{base}/token"))
        .send_form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client),
            ("redirect_uri", callback),
            ("code", &code),
            ("code_verifier", verifier),
            ("resource", &resource),
        ])
        .unwrap();
    let tokens: Value = serde_json::from_str(&response.into_string().unwrap()).unwrap();
    tokens["access_token"].as_str().unwrap().into()
}
