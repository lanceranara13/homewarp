//! What Core and the Gate say to each other (PLAN.md §5.4 and §5.5).
//!
//! Types and nothing else. The Gate is the part of Homewarp that faces the
//! internet, and what it depends on is kept short enough to read.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

/// The protocol a forwarded port speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Tcp,
    Udp,
}

/// A port on the VPS's public address whose traffic is sent home, to the same
/// port there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Forward {
    pub port: u16,
    pub protocol: Protocol,
}

/// Whose address a server at home sees its players come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The player's own. It takes a way back through the tunnel at home.
    #[default]
    Transparent,
    /// The Gate's, for a home where that way back cannot be made to work.
    Nat,
}

/// How many new connections a second the Gate lets one address open through
/// the forwarded ports, unless it is told another number. More are dropped
/// there, before they are sent home.
pub const NEW_PER_SECOND: u32 = 30;

/// All that the Gate is to do. Core sends the whole of it every time, so the
/// Gate never has to work out what changed, and keeps the last one it was
/// sent so that it comes back from a reboot doing the same.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Desired {
    /// Counted up by Core with every change. The Gate says which one it has.
    pub generation: u64,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub forwards: Vec<Forward>,
    /// How many new connections a second one address may open through the
    /// forwarded ports, with twice as many at once. Nothing leaves it at
    /// [`NEW_PER_SECOND`], which is also what a Core that knows of no such
    /// number gets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_per_second: Option<u32>,
}

/// How the Gate is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    /// Which [`Desired`] it is carrying out.
    pub generation: u64,
    pub mode: Mode,
    pub forwards: usize,
    /// How long ago home was last heard from. None if it never was.
    pub handshake_age_seconds: Option<u64>,
    /// Where home's packets come from: its public address, as the VPS sees it.
    #[serde(default)]
    pub home_endpoint: Option<IpAddr>,
    /// Through the tunnel, counted from when its interface was made.
    pub received_bytes: u64,
    pub sent_bytes: u64,
    /// Through each forwarded port. Nothing from a Gate that does not count,
    /// or on a VPS whose nft cannot.
    #[serde(default)]
    pub traffic: Vec<Through>,
}

/// What has gone through one forwarded port, both ways together, counted from
/// when the Gate last set its rules: which it does anew whenever what it is to
/// forward changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Through {
    pub port: u16,
    pub protocol: Protocol,
    pub bytes: u64,
}

/// Asks the Gate to make keys of its own, in place of those its join token
/// brought (`POST /v1/rotate`). Nothing changes until `POST /v1/rotate/commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rotate {
    /// A new preshared key, which home made and which has travelled nowhere
    /// but through the tunnel.
    pub preshared_key: String,
}

/// What the Gate will switch to once it is told that home has this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rotated {
    /// The public half of a key that never leaves the VPS.
    pub public_key: String,
    /// What the Gate will ask of whoever talks to it from then on.
    pub token: String,
}

/// Asks the Gate to send one more port home for a few seconds
/// (`POST /v1/probe`), so that home can see how a connection through it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeRequest {
    /// Where home listens for it, on its tunnel address.
    pub home_port: u16,
    pub seconds: u8,
}

/// The port the Gate opened on the VPS's public address for a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub port: u16,
}

/// The answer to a question a certificate authority asks of the panel's name
/// (`PUT /v1/challenge/<token>`). The name leads to the VPS, so the authority
/// asks there, on port 80 at `/.well-known/acme-challenge/<token>`; Core has
/// the answer, and the Gate puts it where that is served until
/// `DELETE /v1/challenge/<token>`. The answer is no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub answer: String,
}

/// Who serves an answer on the VPS's port 80.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsweredBy {
    /// The Gate itself, which has the port for as long as there are answers.
    Gate,
    /// A web server that had the port already, and has to serve the Gate's
    /// directory of answers at that path.
    WebServer,
}

/// Where an answer was put, and who serves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answering {
    pub by: AnsweredBy,
    /// The directory the answers are files in, named by their tokens.
    pub directory: String,
}

/// What a VPS is handed to become a home's Gate (PLAN.md §5.5): one line of
/// text, made by Core and given to `homewarp-gate join`.
///
/// The key in it has travelled, by a clipboard at the least. So it counts for
/// a quarter of an hour, and on first contact the Gate makes a key of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinToken {
    /// The private key the Gate starts with.
    #[serde(rename = "k")]
    pub private_key: String,
    #[serde(rename = "h")]
    pub home_public_key: String,
    #[serde(rename = "p")]
    pub preshared_key: String,
    /// What the Gate is to ask of whoever talks to it, until it makes its own.
    #[serde(rename = "t")]
    pub token: String,
    /// The UDP port the Gate's WireGuard listens on, which home dials.
    #[serde(rename = "w")]
    pub wg_port: u16,
    /// Where the Gate answers Core, on its tunnel address.
    #[serde(rename = "a")]
    pub api_port: u16,
    /// The two ends' addresses inside the tunnel.
    #[serde(rename = "g")]
    pub gate_address: Ipv4Addr,
    #[serde(rename = "o")]
    pub home_address: Ipv4Addr,
    /// In Unix seconds.
    #[serde(rename = "x")]
    pub expires_at: u64,
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

impl JoinToken {
    /// The token as one word that survives a shell and a terminal: its JSON in
    /// the base64 that uses neither `+` nor `/`.
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("a token is plain text and numbers");
        let mut text = String::with_capacity(json.len().div_ceil(3) * 4);
        for chunk in json.chunks(3) {
            let bits = chunk
                .iter()
                .fold(0u32, |bits, byte| (bits << 8) | u32::from(*byte))
                << (8 * (3 - chunk.len()));
            // Two characters carry the first byte, three the first two, four all three.
            for place in 0..=chunk.len() {
                text.push(char::from(
                    ALPHABET[(bits >> (18 - 6 * place)) as usize & 63],
                ));
            }
        }
        text
    }

    /// Reads what [`JoinToken::encode`] wrote. None for anything else.
    pub fn decode(text: &str) -> Option<Self> {
        let mut json = Vec::with_capacity(text.len() / 4 * 3);
        let (mut bits, mut held) = (0u32, 0);
        for symbol in text.trim().bytes() {
            let value = ALPHABET.iter().position(|known| *known == symbol)?;
            bits = (bits << 6) | value as u32;
            held += 6;
            if held >= 8 {
                held -= 8;
                json.push((bits >> held) as u8);
            }
        }
        serde_json::from_slice(&json).ok()
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::JoinToken;

    fn token(text: &str) -> JoinToken {
        JoinToken {
            private_key: "aKey+with/both==".to_owned(),
            home_public_key: "h".to_owned(),
            preshared_key: "p".to_owned(),
            token: text.to_owned(),
            wg_port: 51820,
            api_port: 4857,
            gate_address: Ipv4Addr::new(10, 213, 77, 1),
            home_address: Ipv4Addr::new(10, 213, 77, 2),
            expires_at: 1_800_000_000,
        }
    }

    #[test]
    fn a_join_token_comes_back_as_it_went() {
        // Each length of JSON modulo three ends the encoding differently.
        for text in ["", "a", "ab", "abc"] {
            let token = token(text);
            let word = token.encode();
            assert!(
                word.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            );
            assert_eq!(JoinToken::decode(&format!("  {word}\n")), Some(token));
        }
    }

    #[test]
    fn what_is_not_a_join_token_is_not_read_as_one() {
        for odd in ["", "nonsense", "e30", "with spaces in it", "ünicode"] {
            assert_eq!(JoinToken::decode(odd), None);
        }
    }
}
