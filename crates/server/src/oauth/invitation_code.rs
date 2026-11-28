const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const SYMBOLS: usize = 12;

pub(super) fn sample(
    mut next_byte: impl FnMut() -> std::io::Result<u8>,
) -> std::io::Result<String> {
    let mut code = String::with_capacity(SYMBOLS + 2);
    for index in 0..SYMBOLS {
        if index == 4 || index == 8 {
            code.push('-');
        }
        let limit = 256 / ALPHABET.len() * ALPHABET.len();
        loop {
            let byte = usize::from(next_byte()?);
            if byte < limit {
                code.push(char::from(ALPHABET[byte % ALPHABET.len()]));
                break;
            }
        }
    }
    Ok(code)
}

pub(super) fn normalize(code: &str) -> Option<String> {
    let mut normalized = String::with_capacity(SYMBOLS);
    for byte in code.bytes() {
        if matches!(byte, b' ' | b'-' | b'_') {
            continue;
        }
        let symbol = byte.to_ascii_uppercase();
        if normalized.len() == SYMBOLS || !ALPHABET.contains(&symbol) {
            return None;
        }
        normalized.push(char::from(symbol));
    }
    (normalized.len() == SYMBOLS).then_some(normalized)
}
