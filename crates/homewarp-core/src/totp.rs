//! The second step of a sign-in (PLAN.md §6, §11 Phase 5): a code from an
//! authenticator app, as RFC 6238 has it. The app and Homewarp share a secret,
//! and each works the same six digits out of it and of the half minute it is.
//!
//! An account that has lost its app signs in with one of the recovery codes it
//! was given when it turned this on. Each of those works once.

use anyhow::Context;
use sha1::{Digest, Sha1};
use sqlx::SqlitePool;

use crate::auth;

/// How long a code lasts, in seconds.
const STEP: i64 = 30;
/// The base32 of RFC 4648, which is how authenticator apps take a secret.
const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// How many recovery codes an account is given.
const RECOVERY_CODES: usize = 8;

/// A new secret: 160 bits, the length the algorithm is made for, in base32.
pub(crate) fn new_secret() -> anyhow::Result<String> {
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).context("asking the system for randomness")?;
    let (mut bits, mut held, mut text) = (0u32, 0, String::with_capacity(32));
    for byte in bytes {
        bits = (bits << 8) | u32::from(byte);
        held += 8;
        while held >= 5 {
            held -= 5;
            text.push(char::from(ALPHABET[(bits >> held) as usize & 31]));
        }
    }
    Ok(text)
}

fn key(secret: &str) -> Option<Vec<u8>> {
    let (mut bits, mut held, mut bytes) = (0u32, 0, Vec::with_capacity(20));
    for symbol in secret.bytes() {
        let value = ALPHABET.iter().position(|known| *known == symbol)?;
        bits = (bits << 5) | value as u32;
        held += 5;
        if held >= 8 {
            held -= 8;
            bytes.push((bits >> held) as u8);
        }
    }
    Some(bytes)
}

/// HMAC-SHA1, as RFC 2104 has it. Written out because it is twelve lines, and
/// SHA-1 is here already.
fn hmac(key: &[u8], message: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner = Sha1::new()
        .chain_update(block.map(|byte| byte ^ 0x36))
        .chain_update(message)
        .finalize();
    Sha1::new()
        .chain_update(block.map(|byte| byte ^ 0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// The six digits for one half minute.
fn code(key: &[u8], step: i64) -> u32 {
    let hash = hmac(key, &step.to_be_bytes());
    let at = usize::from(hash[19] & 0x0f);
    let word = u32::from_be_bytes([hash[at], hash[at + 1], hash[at + 2], hash[at + 3]]);
    (word & 0x7fff_ffff) % 1_000_000
}

/// The half minute a typed code is the code of, if it is that of this one or
/// of either beside it, which is what a clock a little off needs, and comes
/// after `last`: a code is taken once.
pub(crate) fn passes(secret: &str, typed: &str, now: i64, last: i64) -> Option<i64> {
    let typed: u32 = match typed.trim() {
        digits if digits.len() == 6 && digits.bytes().all(|c| c.is_ascii_digit()) => {
            digits.parse().ok()?
        }
        _ => return None,
    };
    let key = key(secret)?;
    let step = now.div_euclid(STEP);
    // Every one of the three is worked out, so that how long it takes does
    // not say which it was.
    let mut found = None;
    for near in [step - 1, step, step + 1] {
        if code(&key, near) == typed && near > last {
            found = Some(near);
        }
    }
    found
}

/// What an authenticator app takes for a secret, as a link or a picture of one.
pub(crate) fn uri(secret: &str, username: &str) -> String {
    format!("otpauth://totp/Homewarp:{username}?secret={secret}&issuer=Homewarp")
}

/// New recovery codes: as they are shown, once, and as they are kept.
pub(crate) fn new_recovery_codes() -> (Vec<String>, String) {
    let codes: Vec<String> = (0..RECOVERY_CODES)
        .map(|_| auth::new_setup_code())
        .collect();
    let kept: Vec<String> = codes.iter().map(|code| kept_as(code)).collect();
    (codes, serde_json::to_string(&kept).unwrap_or_default())
}

/// A recovery code as the database keeps it: its hash, so that a copy of the
/// database is not a set of working codes.
fn kept_as(code: &str) -> String {
    let hash = auth::token_hash(&code.trim().to_uppercase());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What an account's second step came to.
pub(crate) enum Step {
    /// The account has no second step, or the code was right.
    Passed,
    /// It has one, and no code was given.
    Wanted,
    /// A code was given, and was not right.
    Wrong,
}

/// Takes an account through its second step, if it has one. A code that
/// passes is used up: the half minute it was for, or the recovery code.
pub(crate) async fn second_step(
    db: &SqlitePool,
    user_id: i64,
    typed: &str,
) -> anyhow::Result<Step> {
    let (secret, last, recovery): (Option<String>, i64, String) =
        sqlx::query_as("SELECT totp_secret, totp_step, recovery_codes FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(db)
            .await?;
    let Some(secret) = secret else {
        return Ok(Step::Passed);
    };
    if typed.trim().is_empty() {
        return Ok(Step::Wanted);
    }
    if let Some(step) = passes(&secret, typed, auth::now(), last) {
        sqlx::query("UPDATE users SET totp_step = ? WHERE id = ?")
            .bind(step)
            .bind(user_id)
            .execute(db)
            .await?;
        return Ok(Step::Passed);
    }
    let mut left: Vec<String> =
        serde_json::from_str(&recovery).context("reading an account's recovery codes")?;
    let typed = kept_as(typed);
    let Some(used) = left.iter().position(|kept| *kept == typed) else {
        return Ok(Step::Wrong);
    };
    left.remove(used);
    sqlx::query("UPDATE users SET recovery_codes = ? WHERE id = ?")
        .bind(serde_json::to_string(&left)?)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(Step::Passed)
}

#[cfg(test)]
mod tests {
    use super::{code, key, new_secret, passes, uri};

    /// The secret RFC 6238 tests with, "12345678901234567890", in base32.
    const SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn works_out_the_codes_the_rfc_gives() {
        let key = key(SECRET).unwrap();
        assert_eq!(key, b"12345678901234567890");
        // Its table has eight digits. An app shows the last six.
        for (at, eight) in [
            (59, 94_287_082),
            (1_111_111_109, 7_081_804),
            (1_111_111_111, 14_050_471),
            (1_234_567_890, 89_005_924),
            (2_000_000_000, 69_279_037),
        ] {
            assert_eq!(code(&key, at / 30), eight % 1_000_000, "at {at}");
        }
    }

    #[test]
    fn a_code_is_good_for_its_half_minute_and_those_beside_it_and_once() {
        let now = 1_111_111_109;
        let step = now / 30;
        assert_eq!(passes(SECRET, "081804", now, 0), Some(step));
        assert_eq!(passes(SECRET, " 081804 ", now, 0), Some(step));
        // A clock half a minute off, either way.
        assert_eq!(passes(SECRET, "081804", now + 30, 0), Some(step));
        assert_eq!(passes(SECRET, "081804", now - 30, 0), Some(step));
        assert_eq!(passes(SECRET, "081804", now + 60, 0), None);
        // Taken once: not again, and neither is an earlier one after it.
        assert_eq!(passes(SECRET, "081804", now, step), None);
        assert_eq!(passes(SECRET, "081804", now, step + 1), None);
        for wrong in ["081805", "81804", "0818040", "abcdef", ""] {
            assert_eq!(passes(SECRET, wrong, now, 0), None, "{wrong:?}");
        }
    }

    #[test]
    fn a_new_secret_is_one_an_app_takes() {
        let secret = new_secret().unwrap();
        assert_eq!(secret.len(), 32);
        assert_eq!(key(&secret).unwrap().len(), 20);
        assert_ne!(secret, new_secret().unwrap());
        assert_eq!(
            uri("ABC", "alice"),
            "otpauth://totp/Homewarp:alice?secret=ABC&issuer=Homewarp"
        );
    }
}
