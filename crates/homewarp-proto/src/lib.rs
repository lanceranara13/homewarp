//! What Core and the Gate say to each other (PLAN.md §5.4).
//!
//! Types and nothing else. The Gate is the part of Homewarp that faces the
//! internet, and what it depends on is kept short enough to read.

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
    /// Through the tunnel, counted from when its interface was made.
    pub received_bytes: u64,
    pub sent_bytes: u64,
}
