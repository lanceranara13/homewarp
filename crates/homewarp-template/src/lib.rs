//! Game and app templates.
//!
//! A template says how to install and run one game or app: which images, which
//! startup command, which variables, which config files to patch before a start.
//! Templates are imported from Pterodactyl and Pelican "eggs" (PLAN.md §5.6).
//!
//! Nothing here touches the disk or the network. It turns untrusted text into
//! checked values, which keeps it cheap to test and to fuzz.

mod egg;
pub mod properties;

use serde::{Deserialize, Serialize};

pub use egg::{ImportError, import};

/// How to install and run one game or app. Core keeps a template as this, in
/// JSON, so a field added later needs a default that old documents can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub description: String,
    /// In the egg's order; the first is the default.
    pub images: Vec<Image>,
    /// The command that starts the server, with `{{NAME}}` placeholders.
    pub startup: String,
    /// The server has started once its console prints any of these.
    pub done: Vec<String>,
    pub stop: Stop,
    pub config_files: Vec<ConfigFile>,
    pub install: Option<Install>,
    pub variables: Vec<Variable>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub label: String,
    pub image: String,
}

/// How to ask a server to shut down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    /// Type this into the console.
    Command(String),
    /// Send this signal, such as `SIGINT`.
    Signal(String),
}

/// A file in the server directory to patch before each start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFile {
    pub path: String,
    pub parser: Parser,
    /// Key and new value, in the egg's order. Values may hold `{{...}}` placeholders.
    pub find: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Parser {
    Properties,
    Yaml,
    Json,
    Ini,
    Xml,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Install {
    pub image: String,
    pub entrypoint: String,
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub description: String,
    /// The environment variable the server sees.
    pub env: String,
    pub default: String,
    /// Laravel-style validation rules, one per entry.
    pub rules: Vec<String>,
    pub user_viewable: bool,
    pub user_editable: bool,
}

/// Replaces each `{{NAME}}` that `lookup` knows. A placeholder it does not know
/// is left as written, for the caller to notice or for the image to resolve.
pub fn substitute(text: &str, mut lookup: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + len + 2;
        out.push_str(&rest[..start]);
        match lookup(rest[start + 2..end - 2].trim()) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[start..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::substitute;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "SERVER_JARFILE" => Some("server.jar".to_owned()),
            "SERVER_PORT" => Some("25565".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn fills_in_known_placeholders() {
        assert_eq!(
            substitute(
                "java -jar {{SERVER_JARFILE}} --port {{ SERVER_PORT }}",
                lookup
            ),
            "java -jar server.jar --port 25565"
        );
    }

    #[test]
    fn leaves_unknown_and_unfinished_placeholders_alone() {
        assert_eq!(
            substitute("run {{NOPE}} {{SERVER_PORT}}", lookup),
            "run {{NOPE}} 25565"
        );
        assert_eq!(substitute("run {{SERVER_PORT", lookup), "run {{SERVER_PORT");
    }
}
