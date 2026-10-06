//! Passwords, session tokens and the first-run setup code (PLAN.md §6).

use std::time::{SystemTime, UNIX_EPOCH};

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::http::{HeaderMap, HeaderValue, header::COOKIE};
use sha2::{Digest, Sha256};

const COOKIE_NAME: &str = "homewarp_session";
pub const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;

/// No I, O, 0 or 1: the code is read off a log and typed by hand.
const CODE_SYMBOLS: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Now, in Unix seconds.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the operating system has a source of randomness");
    bytes
}

/// Hashes a password with Argon2id and a fresh salt. Slow by design: call it off
/// the async threads.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let hash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|error| anyhow::anyhow!("hashing a password: {error}"))?;
    Ok(hash.to_string())
}

/// Slow by design, like [`hash_password`].
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

/// A new session token: what the browser's cookie holds.
pub fn new_token() -> String {
    random::<32>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A random (version 4) UUID, written the usual way. It names a server's
/// directory and containers, and is what eggs are given as `P_SERVER_UUID`.
pub fn new_uuid() -> String {
    let mut bytes = random::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// What the database holds in place of a token.
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// The session token a request carries, if any.
pub fn token_from(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| cookie.trim().strip_prefix(COOKIE_NAME)?.strip_prefix('='))
}

/// The `Set-Cookie` value for a session. An empty token and no lifetime clear it.
///
/// `HttpOnly` keeps the token from scripts; `SameSite=Strict` keeps other sites
/// from making the browser send it. There is no `Secure`: the panel is served
/// over plain HTTP on the home network until TLS exists (PLAN.md §5.9).
pub fn cookie(token: &str, max_age: i64) -> HeaderValue {
    let cookie =
        format!("{COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}");
    HeaderValue::from_str(&cookie).expect("a hex token and a number are valid in a header")
}

/// A fresh setup code: twelve symbols, sixty bits, written `XXXX-XXXX-XXXX`.
pub fn new_setup_code() -> String {
    let symbols = random::<12>().map(|byte| CODE_SYMBOLS[usize::from(byte & 31)]);
    let groups: Vec<&str> = symbols
        .chunks(4)
        .filter_map(|group| std::str::from_utf8(group).ok())
        .collect();
    groups.join("-")
}

/// Whether what was typed is the code, forgiving case, dashes and spaces. Takes
/// the same time wherever the two first differ.
pub fn setup_code_matches(typed: &str, code: &str) -> bool {
    let symbols = |text: &str| -> Vec<u8> {
        text.bytes()
            .filter(u8::is_ascii_alphanumeric)
            .map(|byte| byte.to_ascii_uppercase())
            .collect()
    };
    let (typed, code) = (symbols(typed), symbols(code));
    typed.len() == code.len()
        && typed
            .iter()
            .zip(&code)
            .fold(0, |differences, (a, b)| differences | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_verifies_against_its_own_hash_only() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("correct horse batterz", &hash));
        assert!(!verify_password("anything", "not a hash"));
    }

    #[test]
    fn setup_codes_are_typed_by_people() {
        let code = new_setup_code();
        assert_eq!(code.len(), 14);
        assert!(code.split('-').all(|group| group.len() == 4));
        assert_ne!(code, new_setup_code());

        assert!(setup_code_matches(
            &code.to_lowercase().replace('-', " "),
            &code
        ));
        assert!(!setup_code_matches(&code[..13], &code));
        assert!(!setup_code_matches("", &code));
    }

    #[test]
    fn finds_the_session_cookie_among_others() {
        let mut headers = HeaderMap::new();
        assert_eq!(token_from(&headers), None);
        headers.insert(
            COOKIE,
            HeaderValue::from_static("theme=dark; homewarp_session=abc123; other=1"),
        );
        assert_eq!(token_from(&headers), Some("abc123"));
        headers.insert(COOKIE, HeaderValue::from_static("homewarp_session_old=zzz"));
        assert_eq!(token_from(&headers), None);
    }

    #[test]
    fn tokens_are_long_and_stored_hashed() {
        let token = new_token();
        assert_eq!(token.len(), 64);
        assert_ne!(token, new_token());
        assert_eq!(token_hash(&token).len(), 32);
        assert_eq!(token_hash(&token), token_hash(&token));
    }
}
