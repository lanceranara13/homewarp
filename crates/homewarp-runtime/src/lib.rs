//! Runs servers as Docker containers (PLAN.md §5.6).
//!
//! It keeps the contract Pterodactyl's Wings gives a container, so unmodified
//! eggs and "yolk" images work: the server's files at `/home/container`, the
//! install script run from `/mnt/install` with the files at `/mnt/server`, and
//! the startup command in `$STARTUP`.

mod console;
mod engine;
mod files;

pub use console::{Console, strip_ansi};
pub use engine::{Engine, Error, InstallScript, Listener, Network, Port, Protocol, Server, Usage};
pub use files::ServerDir;
