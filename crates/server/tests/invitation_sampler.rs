#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/oauth/invitation_code.rs"]
mod invitation_code;

#[test]
fn the_code_has_no_modulo_bias() {
    let alphabet = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
    let mut observed = Vec::new();
    let mut bytes = (0..=255).chain(0..=255);
    for _ in 0..21 {
        let code = invitation_code::sample(|| Ok(bytes.next().unwrap())).unwrap();
        let normalized = invitation_code::normalize(&code).unwrap();
        observed.extend(normalized.bytes());
    }
    assert_eq!(
        observed,
        alphabet
            .repeat(8)
            .into_iter()
            .chain(*b"2345")
            .collect::<Vec<_>>()
    );
    assert_eq!(bytes.next(), Some(4));
}
