//! Passkeys (PLAN.md §11, Phase 5): what a browser's authenticator says, read
//! and checked.
//!
//! A passkey is a key that never leaves the device it was made on. The device
//! proves it has the key by signing a challenge, and it signs only for the
//! site the key was made for, which the browser sees to: that is what a
//! password typed into a page that looks like the panel cannot give away.
//!
//! Only what Homewarp asks for is understood here: keys on the P-256 curve
//! (ES256, which every authenticator can make), a device that checked who was
//! holding it, and no attestation, since which make of device it is matters
//! to nobody. Nothing here keeps anything: it reads bytes and says yes or no.

use ring::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// What an authenticator sets in the byte of flags: that someone was there,
/// that it checked who, and that the data carries a new credential.
const PRESENT: u8 = 0x01;
const VERIFIED: u8 = 0x04;
const ATTESTED: u8 = 0x40;
/// How deep a value may nest. What an authenticator sends is three deep.
const DEEPEST: u8 = 6;
/// The longest a credential's identifier is allowed to be.
const LONGEST_ID: usize = 1023;

/// Why something was refused. Said to whoever sent it, so it tells nothing
/// that helps a forger: only which part did not hold.
pub(crate) type Refused = &'static str;

/// One value of CBOR, of the kinds an authenticator uses.
#[derive(Debug, PartialEq)]
enum Value<'a> {
    Number(i64),
    Bytes(&'a [u8]),
    Text(&'a str),
    Array(Vec<Value<'a>>),
    Map(Vec<(Value<'a>, Value<'a>)>),
    /// True, false, nothing, or a number with a fraction: read past, not kept.
    Other,
}

impl<'a> Value<'a> {
    /// What a map holds under a number, as the keys of a public key are.
    fn at(&self, key: i64) -> Option<&Value<'a>> {
        let Self::Map(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find_map(|(name, value)| (*name == Value::Number(key)).then_some(value))
    }

    /// What a map holds under a word.
    fn named(&self, key: &str) -> Option<&Value<'a>> {
        let Self::Map(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find_map(|(name, value)| (*name == Value::Text(key)).then_some(value))
    }

    fn bytes(&self) -> Option<&'a [u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }
}

/// Takes `count` bytes off the front of what is left.
fn take<'a>(rest: &mut &'a [u8], count: usize) -> Option<&'a [u8]> {
    let (taken, after) = rest.split_at_checked(count)?;
    *rest = after;
    Some(taken)
}

/// Reads one value off the front of `rest`. None for anything that is not
/// CBOR as an authenticator writes it: of a set length, and not too deep.
fn value<'a>(rest: &mut &'a [u8], depth: u8) -> Option<Value<'a>> {
    let first = *take(rest, 1)?.first()?;
    let (kind, short) = (first >> 5, first & 0x1f);
    // The number that follows the kind: how much, how long or how many.
    let amount = match short {
        0..=23 => u64::from(short),
        24..=27 => {
            let bytes = take(rest, 1 << (short - 24))?;
            bytes
                .iter()
                .fold(0, |amount, byte| (amount << 8) | u64::from(*byte))
        }
        _ => return None,
    };
    let length = usize::try_from(amount).ok();
    Some(match kind {
        0 => Value::Number(i64::try_from(amount).ok()?),
        1 => Value::Number(-1 - i64::try_from(amount).ok()?),
        2 => Value::Bytes(take(rest, length?)?),
        3 => Value::Text(std::str::from_utf8(take(rest, length?)?).ok()?),
        4 | 5 if depth == 0 => return None,
        4 => {
            // No more values than there are bytes left to hold them.
            let count = length.filter(|count| *count <= rest.len())?;
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(value(rest, depth - 1)?);
            }
            Value::Array(values)
        }
        5 => {
            let count = length.filter(|count| *count <= rest.len() / 2)?;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                entries.push((value(rest, depth - 1)?, value(rest, depth - 1)?));
            }
            Value::Map(entries)
        }
        // A tag says what kind of thing follows, which nothing here asks.
        6 if depth > 0 => value(rest, depth - 1)?,
        // For this kind the number was the value itself, and is all of it.
        7 => Value::Other,
        _ => return None,
    })
}

/// What a request is held against: the challenge that was handed out for it,
/// and the site it has to have been made on.
pub(crate) struct Expected<'a> {
    /// As it was handed out, in the base64 that uses neither `+` nor `/`.
    pub(crate) challenge: &'a str,
    /// The page's origin, such as `https://panel.example.com:8443`.
    pub(crate) origin: &'a str,
    /// The name the key is bound to: the host of that origin.
    pub(crate) rp_id: &'a str,
}

/// What the browser says it was asked, in its own JSON.
#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

/// The challenge the browser says it answered, for whoever has to find out
/// first whether that challenge is out.
pub(crate) fn challenge(client_data: &[u8]) -> Option<String> {
    let said: ClientData = serde_json::from_slice(client_data).ok()?;
    Some(said.challenge)
}

/// Checks that the browser was asked this, for this challenge, on this site.
fn asked(client_data: &[u8], kind: &str, expected: &Expected<'_>) -> Result<(), Refused> {
    let said: ClientData = serde_json::from_slice(client_data)
        .map_err(|_| "What the browser sent could not be read.")?;
    if said.kind != kind {
        return Err("That was made for something else.");
    }
    if said.challenge != expected.challenge {
        return Err("That answers a challenge that was not handed out.");
    }
    if said.origin != expected.origin || said.cross_origin {
        return Err("That was made on another site.");
    }
    Ok(())
}

/// What an authenticator says of itself, ahead of anything else it sends.
struct Said<'a> {
    flags: u8,
    /// How many times the key has been used, where the device counts.
    count: u32,
    /// What follows: a new credential, where one was made.
    rest: &'a [u8],
}

/// Reads that, and checks that it is for this site, that someone was there,
/// and that the device checked who.
fn said<'a>(authenticator_data: &'a [u8], expected: &Expected<'_>) -> Result<Said<'a>, Refused> {
    let short = "What the authenticator sent is cut short.";
    let mut rest = authenticator_data;
    let site = take(&mut rest, 32).ok_or(short)?;
    let flags = *take(&mut rest, 1).and_then(<[u8]>::first).ok_or(short)?;
    let count = take(&mut rest, 4).ok_or(short)?;
    if site != Sha256::digest(expected.rp_id).as_slice() {
        return Err("That passkey is for another site.");
    }
    if flags & PRESENT == 0 {
        return Err("Nobody was there to confirm it.");
    }
    if flags & VERIFIED == 0 {
        return Err(
            "The device did not check who was holding it. A passkey here needs its PIN, fingerprint or face.",
        );
    }
    Ok(Said {
        flags,
        count: u32::from_be_bytes([count[0], count[1], count[2], count[3]]),
        rest,
    })
}

/// A passkey as it is kept.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Passkey {
    /// What the device calls it. A browser sends this back to say which key signed.
    pub(crate) id: Vec<u8>,
    /// The public half: a point on the curve, written out in full.
    pub(crate) public_key: Vec<u8>,
    pub(crate) count: u32,
}

/// Reads a passkey that a browser has just had made, and checks that it was
/// made for this challenge on this site.
pub(crate) fn made(
    client_data: &[u8],
    attestation: &[u8],
    expected: &Expected<'_>,
) -> Result<Passkey, Refused> {
    let unread = "What the authenticator sent could not be read.";
    asked(client_data, "webauthn.create", expected)?;
    let mut rest = attestation;
    let attestation = value(&mut rest, DEEPEST).ok_or(unread)?;
    let data = attestation
        .named("authData")
        .and_then(Value::bytes)
        .ok_or(unread)?;
    let said = said(data, expected)?;
    if said.flags & ATTESTED == 0 {
        return Err("The authenticator sent no key.");
    }
    let mut rest = said.rest;
    // Which make of device it is: sixteen bytes that nothing here asks after.
    take(&mut rest, 16).ok_or(unread)?;
    let length = take(&mut rest, 2).ok_or(unread)?;
    let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
    if length == 0 || length > LONGEST_ID {
        return Err(unread);
    }
    let id = take(&mut rest, length).ok_or(unread)?;
    let key = value(&mut rest, DEEPEST).ok_or(unread)?;
    // An elliptic-curve key (2), for ES256 (-7), on P-256 (1).
    let kind = (key.at(1), key.at(3), key.at(-1));
    if kind
        != (
            Some(&Value::Number(2)),
            Some(&Value::Number(-7)),
            Some(&Value::Number(1)),
        )
    {
        return Err("That kind of key is not one Homewarp takes.");
    }
    let half = |at: i64| {
        key.at(at)
            .and_then(Value::bytes)
            .filter(|half| half.len() == 32)
    };
    let (x, y) = half(-2).zip(half(-3)).ok_or(unread)?;
    Ok(Passkey {
        id: id.to_vec(),
        public_key: [&[0x04][..], x, y].concat(),
        count: said.count,
    })
}

/// Checks a sign-in made with a passkey: that the device holding the key
/// signed this challenge, for this site. Returns how many times the key has
/// been used, as the device counts.
pub(crate) fn signed(
    client_data: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
    public_key: &[u8],
    expected: &Expected<'_>,
) -> Result<u32, Refused> {
    asked(client_data, "webauthn.get", expected)?;
    let said = said(authenticator_data, expected)?;
    // What the device signs: what it says of itself, and a digest of what the
    // browser says it was asked.
    let message = [authenticator_data, Sha256::digest(client_data).as_slice()].concat();
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, public_key)
        .verify(&message, signature)
        .map_err(|_| "That was not signed by the passkey.")?;
    Ok(said.count)
}

/// A device that makes passkeys and signs with them, as the tests need one.
#[cfg(test)]
pub(crate) mod device {
    use ring::{
        rand::SystemRandom,
        signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair},
    };
    use sha2::{Digest, Sha256};

    use super::{ATTESTED, PRESENT, VERIFIED};

    pub(crate) struct Device {
        key: EcdsaKeyPair,
        pub(crate) id: Vec<u8>,
        pub(crate) count: u32,
        /// What it sets of the flags. A device that checks nobody leaves one out.
        pub(crate) flags: u8,
    }

    /// A byte string as CBOR writes one.
    fn bytes(of: &[u8]) -> Vec<u8> {
        let head = match of.len() {
            short @ 0..=23 => vec![0x40 | short as u8],
            byte @ 24..=255 => vec![0x58, byte as u8],
            long => vec![0x59, (long >> 8) as u8, long as u8],
        };
        [head.as_slice(), of].concat()
    }

    pub(crate) fn client_data(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
        format!(r#"{{"type":"{kind}","challenge":"{challenge}","origin":"{origin}","crossOrigin":false}}"#)
            .into_bytes()
    }

    impl Device {
        pub(crate) fn new() -> Self {
            let random = SystemRandom::new();
            let made =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &random).unwrap();
            let key =
                EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, made.as_ref(), &random)
                    .unwrap();
            Self {
                id: Sha256::digest(key.public_key().as_ref())[..20].to_vec(),
                key,
                count: 0,
                flags: PRESENT | VERIFIED,
            }
        }

        fn says(&mut self, rp_id: &str, flags: u8) -> Vec<u8> {
            self.count += 1;
            [
                Sha256::digest(rp_id).as_slice(),
                &[flags],
                &self.count.to_be_bytes(),
            ]
            .concat()
        }

        /// What it answers when a passkey is made: the attestation object.
        pub(crate) fn make(&mut self, rp_id: &str) -> Vec<u8> {
            let point = self.key.public_key().as_ref().to_vec();
            let key = [
                // A map of five: kind 2, algorithm -7, curve 1, and the two halves.
                &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21][..],
                &bytes(&point[1..33]),
                &[0x22],
                &bytes(&point[33..]),
            ]
            .concat();
            let data = [
                self.says(rp_id, self.flags | ATTESTED).as_slice(),
                &[0; 16],
                &(self.id.len() as u16).to_be_bytes(),
                &self.id,
                &key,
            ]
            .concat();
            [
                // A map of three: no attestation, an empty statement, and the data.
                &[0xa3, 0x63][..],
                b"fmt",
                &[0x64],
                b"none",
                &[0x67],
                b"attStmt",
                &[0xa0, 0x68],
                b"authData",
                &bytes(&data),
            ]
            .concat()
        }

        /// What it answers when it is signed in with: what it says of
        /// itself, and its signature over that and what the browser was asked.
        pub(crate) fn sign(&mut self, rp_id: &str, client_data: &[u8]) -> (Vec<u8>, Vec<u8>) {
            let data = self.says(rp_id, self.flags);
            let message = [data.as_slice(), Sha256::digest(client_data).as_slice()].concat();
            let signature = self.key.sign(&SystemRandom::new(), &message).unwrap();
            (data, signature.as_ref().to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEEPEST, Expected, PRESENT, Value,
        device::{Device, client_data},
        made, signed, value,
    };

    const HERE: Expected<'static> = Expected {
        challenge: "c29tZSBjaGFsbGVuZ2U",
        origin: "https://panel.example.com:8443",
        rp_id: "panel.example.com",
    };

    fn read(mut bytes: &[u8]) -> Option<Value<'_>> {
        value(&mut bytes, DEEPEST)
    }

    #[test]
    fn what_an_authenticator_writes_is_read() {
        assert_eq!(read(&[0x00]), Some(Value::Number(0)));
        assert_eq!(read(&[0x18, 0x64]), Some(Value::Number(100)));
        assert_eq!(read(&[0x19, 0x03, 0xe8]), Some(Value::Number(1000)));
        assert_eq!(read(&[0x26]), Some(Value::Number(-7)));
        assert_eq!(read(&[0x38, 0x63]), Some(Value::Number(-100)));
        assert_eq!(read(&[0x43, 1, 2, 3]), Some(Value::Bytes(&[1, 2, 3])));
        assert_eq!(read(b"\x64none"), Some(Value::Text("none")));
        assert_eq!(
            read(&[0x82, 0x01, 0x41, 0x09]),
            Some(Value::Array(vec![Value::Number(1), Value::Bytes(&[9])]))
        );
        let map = read(&[0xa2, 0x01, 0x02, 0x61, b'a', 0xf5]).unwrap();
        assert_eq!(map.at(1), Some(&Value::Number(2)));
        assert_eq!(map.named("a"), Some(&Value::Other));
        assert_eq!(map.at(2), None);
    }

    #[test]
    fn what_is_not_that_is_refused_and_never_trusted_for_its_size() {
        for odd in [
            &[][..],
            // Cut short: a string of three with two there, a number with no bytes.
            &[0x43, 1, 2],
            &[0x19, 0x03],
            // Of no set length, which an authenticator never writes.
            &[0x5f, 0x41, 1, 0xff],
            &[0x9f, 0x01, 0xff],
            // A map that says it holds more than there are bytes for.
            &[0xbb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            &[0x9a, 0xff, 0xff, 0xff, 0xff, 0x01],
            // Text that is not text.
            &[0x62, 0xff, 0xfe],
        ] {
            assert_eq!(read(odd), None, "{odd:?}");
        }
        // Nested deeper than anything real: each array holds the next.
        let deep = [vec![0x81; 40], vec![0x00]].concat();
        assert_eq!(read(&deep), None);
    }

    #[test]
    fn a_passkey_is_made_and_signs_in_on_the_site_it_was_made_for() {
        let mut device = Device::new();
        let asked = client_data("webauthn.create", HERE.challenge, HERE.origin);
        let passkey = made(&asked, &device.make(HERE.rp_id), &HERE).unwrap();
        assert_eq!(passkey.id, device.id);
        assert_eq!((passkey.public_key.len(), passkey.public_key[0]), (65, 4));
        assert_eq!(passkey.count, 1);

        let asked = client_data("webauthn.get", HERE.challenge, HERE.origin);
        let (data, signature) = device.sign(HERE.rp_id, &asked);
        assert_eq!(
            signed(&asked, &data, &signature, &passkey.public_key, &HERE),
            Ok(2)
        );

        // Another device's signature is not this key's.
        let (_, forged) = Device::new().sign(HERE.rp_id, &asked);
        assert!(signed(&asked, &data, &forged, &passkey.public_key, &HERE).is_err());
        // Nor is this one's, over anything but what was sent.
        let mut changed = data.clone();
        changed[36] ^= 1;
        assert!(signed(&asked, &changed, &signature, &passkey.public_key, &HERE).is_err());
    }

    #[test]
    fn what_was_made_elsewhere_or_for_another_challenge_is_refused() {
        let mut device = Device::new();
        let made_here = client_data("webauthn.create", HERE.challenge, HERE.origin);
        let passkey = made(&made_here, &device.make(HERE.rp_id), &HERE).unwrap();

        for (kind, challenge, origin, rp_id, why) in [
            // A page that looks like the panel, on a name that is not its.
            (
                "webauthn.get",
                HERE.challenge,
                "https://panel.example.net:8443",
                HERE.rp_id,
                "another site",
            ),
            // The right page, and a key that was made for another.
            (
                "webauthn.get",
                HERE.challenge,
                HERE.origin,
                "example.net",
                "for another site",
            ),
            // An answer kept from before, to a challenge that is no longer out.
            (
                "webauthn.get",
                "b2xkZXI",
                HERE.origin,
                HERE.rp_id,
                "not handed out",
            ),
            // What was signed when the key was made, sent as a sign-in.
            (
                "webauthn.create",
                HERE.challenge,
                HERE.origin,
                HERE.rp_id,
                "something else",
            ),
        ] {
            let asked = client_data(kind, challenge, origin);
            let (data, signature) = device.sign(rp_id, &asked);
            let refused =
                signed(&asked, &data, &signature, &passkey.public_key, &HERE).unwrap_err();
            assert!(refused.contains(why), "{refused}");
        }

        // A device that checked nobody, or that nobody touched.
        let asked = client_data("webauthn.get", HERE.challenge, HERE.origin);
        device.flags = PRESENT;
        let (data, signature) = device.sign(HERE.rp_id, &asked);
        let refused = signed(&asked, &data, &signature, &passkey.public_key, &HERE).unwrap_err();
        assert!(refused.contains("did not check who"), "{refused}");
        device.flags = 0;
        let (data, signature) = device.sign(HERE.rp_id, &asked);
        assert!(signed(&asked, &data, &signature, &passkey.public_key, &HERE).is_err());
        assert!(made(&made_here, &device.make(HERE.rp_id), &HERE).is_err());
    }

    #[test]
    fn a_passkey_that_is_cut_short_or_of_another_kind_is_refused() {
        let mut device = Device::new();
        let asked = client_data("webauthn.create", HERE.challenge, HERE.origin);
        let whole = device.make(HERE.rp_id);
        assert!(made(&asked, &whole, &HERE).is_ok());
        // Every shorter beginning of it: refused, and nothing reads past its end.
        for length in 0..whole.len() {
            assert!(made(&asked, &whole[..length], &HERE).is_err(), "{length}");
        }
        // A key for another algorithm: the -7 that says ES256 made -8.
        let mut other = whole.clone();
        let at = other
            .windows(4)
            .position(|bytes| bytes == [0x01, 0x02, 0x03, 0x26])
            .unwrap();
        other[at + 3] = 0x27;
        assert_eq!(
            made(&asked, &other, &HERE),
            Err("That kind of key is not one Homewarp takes.")
        );
        assert!(made(b"not json", &whole, &HERE).is_err());
    }
}
