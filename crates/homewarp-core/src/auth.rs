//! Passwords, session tokens and the first-run setup code (PLAN.md §6).

use std::{
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::http::{HeaderMap, HeaderValue, header::COOKIE};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::api::Problem;

const COOKIE_NAME: &str = "homewarp_session";
pub const SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;
const DEVICE_COOKIE: &str = "homewarp_device";
/// How long a browser that has signed in is known for: a year.
const DEVICE_SECONDS: i64 = 365 * 24 * 60 * 60;

/// How many passwords are hashed at once. Anybody may ask for one to be, by
/// trying to sign in, and each takes a processor and some megabytes for as
/// long as it lasts.
const HASHING_AT_ONCE: usize = 4;
/// How long one waits for its turn before whoever asked is told to come back.
const TURN: Duration = Duration::from_secs(10);
static HASHING: Semaphore = Semaphore::const_new(HASHING_AT_ONCE);

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

/// Does one piece of slow work on a password when its turn comes, off the
/// async threads. Only a few are done at once, however many are asked for,
/// and whether or not those who asked wait for the answer.
async fn in_turn<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Problem> {
    const BUSY: Problem = Problem::Unavailable(
        "Homewarp is checking as many passwords as it does at once. Try again in a moment.",
    );
    let Ok(Ok(turn)) = tokio::time::timeout(TURN, HASHING.acquire()).await else {
        return Err(BUSY);
    };
    // The turn goes with the work and is given back when the work is done.
    // Whoever asked may have gone by then: the work goes on without them, and
    // a turn given back when they went would let any number of it go on at once.
    tokio::task::spawn_blocking(move || {
        let _turn = turn;
        work()
    })
    .await
    .map_err(|error| Problem::Internal(error.into()))
}

/// [`hash_password`], in its turn.
pub(crate) async fn hash(password: String) -> Result<String, Problem> {
    Ok(in_turn(move || hash_password(&password)).await??)
}

/// [`verify_password`], in its turn. With no hash to hold it against, the same
/// work is done for nothing: an unknown name then costs what a wrong password
/// does, and how long the answer takes does not say which of the two it was.
pub(crate) async fn verify(password: String, hash: Option<String>) -> Result<bool, Problem> {
    in_turn(move || {
        static NOBODY: OnceLock<String> = OnceLock::new();
        let hash = match &hash {
            Some(hash) => hash,
            None => NOBODY.get_or_init(|| hash_password("").unwrap_or_default()),
        };
        verify_password(&password, hash)
    })
    .await
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

/// What a request's cookie of this name holds, if it carries one.
fn cookie_named<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| cookie.trim().strip_prefix(name)?.strip_prefix('='))
}

/// The session token a request carries, if any.
pub fn token_from(headers: &HeaderMap) -> Option<&str> {
    cookie_named(headers, COOKIE_NAME)
}

/// The token of a browser that says it has signed in here before, if it says so.
pub fn device_from(headers: &HeaderMap) -> Option<&str> {
    cookie_named(headers, DEVICE_COOKIE)
}

/// The `Set-Cookie` value by which a browser is known from now on. It is of
/// use to a sign-in and to nothing else, so that is all it is sent back with.
pub fn device_cookie(token: &str) -> HeaderValue {
    let cookie = format!(
        "{DEVICE_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/api/v1/login; Max-Age={DEVICE_SECONDS}"
    );
    HeaderValue::from_str(&cookie).expect("a hex token and a number are valid in a header")
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_turn_is_kept_for_as_long_as_the_work_takes_whether_or_not_it_is_waited_for() {
        let (end, ended) = std::sync::mpsc::channel::<()>();
        let (began, begun) = tokio::sync::oneshot::channel();
        let asked = tokio::spawn(in_turn(move || {
            let _ = began.send(());
            let _ = ended.recv();
        }));
        begun.await.unwrap();
        let taken = HASHING.available_permits();
        assert!(taken < HASHING_AT_ONCE);
        // Whoever asked goes away. The work goes on, and so its turn is still taken.
        asked.abort();
        let _ = asked.await;
        assert_eq!(HASHING.available_permits(), taken);
        // The work ends, and the turn is given back.
        drop(end);
        for _ in 0..500 {
            if HASHING.available_permits() == taken + 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the turn was never given back");
    }

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

    #[tokio::test]
    async fn passwords_are_hashed_a_few_at_a_time_and_the_rest_wait() {
        let hash = hash("correct horse battery".to_owned()).await.unwrap();
        // Every turn taken: one more waits, and is done once there is a turn.
        let all = u32::try_from(HASHING_AT_ONCE).unwrap();
        let turns = HASHING.acquire_many(all).await.unwrap();
        assert_eq!(HASHING.available_permits(), 0);
        let mut waiting = tokio::spawn(verify("correct horse battery".to_owned(), Some(hash)));
        let waited = tokio::time::timeout(Duration::from_millis(100), &mut waiting).await;
        assert!(waited.is_err());
        drop(turns);
        assert!(waiting.await.unwrap().unwrap());
        // Held against nobody's password, a guess is wrong, and takes its turn as any does.
        assert!(!verify("a guess".to_owned(), None).await.unwrap());
    }

    #[test]
    fn the_cookie_a_browser_is_known_by_goes_to_sign_ins_only() {
        let cookie = device_cookie("abc123");
        let cookie = cookie.to_str().unwrap();
        assert!(cookie.starts_with("homewarp_device=abc123; HttpOnly; SameSite=Strict; "));
        assert!(cookie.contains("Path=/api/v1/login;"));
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("homewarp_session=s; homewarp_device=abc123"),
        );
        assert_eq!(device_from(&headers), Some("abc123"));
        assert_eq!(token_from(&headers), Some("s"));
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
