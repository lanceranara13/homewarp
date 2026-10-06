//! Patches a server's config files before a start (PLAN.md §5.6).
//!
//! An egg names files, a parser for each, and settings to make in them. This
//! follows what Wings does with those, as read from its source
//! (`parser/parser.go`), with three differences made on purpose:
//!
//! - an INI file keeps its comments and its layout, where Wings writes it afresh;
//! - a JSON file keeps the order of its keys, where Wings sorts them;
//! - a replacement that asks what is there first is made when that is there.
//!   Wings has both of those tests the wrong way round, so they never pass.

use regex_lite::Regex;
use serde::Serialize;
use serde_json::{Map, Value, ser::PrettyFormatter};

use crate::{Parser, Replacement, properties};

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("it is not valid JSON ({0})")]
    Json(#[from] serde_json::Error),
    #[error("it is not valid YAML ({0})")]
    Yaml(String),
    #[error("Homewarp cannot set up XML files yet")]
    Xml,
}

/// The file's text with the replacements made. A file that is not there yet is
/// the empty text, and comes out holding just what was set.
pub fn patch(
    parser: Parser,
    text: &str,
    replacements: &[Replacement],
) -> Result<String, PatchError> {
    Ok(match parser {
        Parser::Properties => {
            let pairs: Vec<(String, String)> = replacements
                .iter()
                .map(|replacement| (replacement.key.clone(), replacement.value.clone()))
                .collect();
            properties::patch(text, &pairs)
        }
        Parser::File => lines(text, replacements),
        Parser::Ini => ini(text, replacements),
        Parser::Json => {
            let mut root = match text.trim() {
                "" => Value::Object(Map::new()),
                text => serde_json::from_str(text)?,
            };
            tree(&mut root, replacements);
            let mut json = Vec::new();
            let four_spaces = PrettyFormatter::with_indent(b"    ");
            root.serialize(&mut serde_json::Serializer::with_formatter(
                &mut json,
                four_spaces,
            ))?;
            String::from_utf8_lossy(&json).into_owned() + "\n"
        }
        Parser::Yaml => {
            let yaml = |error: &dyn std::fmt::Display| {
                PatchError::Yaml(
                    error
                        .to_string()
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                )
            };
            let mut root = match text.trim() {
                "" => Value::Object(Map::new()),
                text => serde_saphyr::from_str(text).map_err(|error| yaml(&error))?,
            };
            tree(&mut root, replacements);
            serde_saphyr::to_string(&root).map_err(|error| yaml(&error))?
        }
        Parser::Xml => return Err(PatchError::Xml),
    })
}

/// The `file` parser: a line that starts with a key is replaced, whole, by its
/// value. A key that no line starts with sets nothing.
fn lines(text: &str, replacements: &[Replacement]) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let replaced = replacements
            .iter()
            .find(|replacement| line.starts_with(replacement.key.as_str()));
        out.push_str(replaced.map_or(line, |replacement| replacement.value.as_str()));
        out.push('\n');
    }
    out
}

/// The `ini` parser: `section.key`, or `key` alone for what comes before any
/// section. A key or a section that is not there is added.
fn ini(text: &str, replacements: &[Replacement]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    for replacement in replacements {
        // The first dot outside brackets parts the section from the key, so a
        // section with a dot in its name is written `[like.this].key`.
        let (mut section, mut key, mut depth) = (None, String::new(), 0);
        for c in replacement.key.chars() {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                '.' if depth == 0 && section.is_none() => section = Some(std::mem::take(&mut key)),
                c => key.push(c),
            }
        }
        set_ini(&mut lines, section.as_deref(), &key, &replacement.value);
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn set_ini(lines: &mut Vec<String>, section: Option<&str>, key: &str, value: &str) {
    let heading = |line: &str| {
        let name = line.trim().strip_prefix('[')?.strip_suffix(']')?;
        Some(name.trim().to_owned())
    };
    let start = match section {
        None => Some(0),
        Some(name) => lines
            .iter()
            .position(|line| heading(line).as_deref() == Some(name))
            .map(|at| at + 1),
    };
    let Some(start) = start else {
        if lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{}]", section.unwrap_or_default()));
        lines.push(format!("{key}={value}"));
        return;
    };
    let end = lines[start..]
        .iter()
        .position(|line| heading(line).is_some())
        .map_or(lines.len(), |length| start + length);

    for line in &mut lines[start..end] {
        if line.trim_start().starts_with([';', '#']) {
            continue;
        }
        let Some(separator) = line.find(['=', ':']) else {
            continue;
        };
        if line[..separator].trim() != key {
            continue;
        }
        // The line stays as it is written, up to where its value begins.
        let written = &line[separator + 1..];
        let gap = written.len() - written.trim_start().len();
        line.truncate(separator + 1 + gap);
        line.push_str(value);
        return;
    }
    // Not there: it goes after the last line of the section that says anything.
    let after = lines[start..end]
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |last| start + last + 1);
    lines.insert(after, format!("{key}={value}"));
}

/// The `json` and `yaml` parsers: a dotted path to a value, in which `*` stands
/// for every child and `name[0]` for an element of the list `name`.
fn tree(root: &mut Value, replacements: &[Replacement]) {
    for replacement in replacements {
        let Some((parent, rest)) = replacement.key.split_once(".*") else {
            set(root, &replacement.key, replacement);
            continue;
        };
        let rest = rest.trim_matches('.');
        match walk_mut(root, parent.trim_matches('.')) {
            Some(Value::Object(children)) => {
                for child in children.values_mut() {
                    set(child, rest, replacement);
                }
            }
            Some(Value::Array(children)) => {
                for child in children {
                    set(child, rest, replacement);
                }
            }
            _ => {}
        }
    }
}

fn set(container: &mut Value, path: &str, replacement: &Replacement) {
    let there = walk(container, path);
    let now = there.map(|value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    });
    let value = match (replacement.only_if.as_deref(), now) {
        (None, _) => typed(&replacement.value, there),
        // A pattern rewrites what is there, so something has to be.
        (Some(test), now) if test.starts_with("regex:") => {
            let (Some(now), Ok(pattern)) = (now, Regex::new(&test["regex:".len()..])) else {
                return;
            };
            if !pattern.is_match(&now) {
                return;
            }
            Value::String(
                pattern
                    .replace_all(&now, replacement.value.as_str())
                    .into_owned(),
            )
        }
        (Some(test), Some(now)) if now != test => return,
        (Some(_), _) => typed(&replacement.value, there),
    };
    put(container, path, value);
}

/// A value as a file of this kind would write it: a number or a truth where
/// that is what the text says, unless the file has text there already.
fn typed(text: &str, there: Option<&Value>) -> Value {
    if matches!(there, Some(Value::String(_))) {
        return Value::String(text.to_owned());
    }
    match (text.parse::<i64>(), text) {
        (Ok(number), _) => Value::Number(number.into()),
        (_, "true") => Value::Bool(true),
        (_, "false") => Value::Bool(false),
        _ => Value::String(text.to_owned()),
    }
}

/// A step of a path: a name, and for `name[2]` the place in the list of that name.
fn step(written: &str) -> (&str, Option<usize>) {
    let place = written
        .strip_suffix(']')
        .and_then(|rest| rest.split_once('['))
        .and_then(|(name, place)| Some((name, place.parse().ok()?)));
    match place {
        Some((name, place)) => (name, Some(place)),
        None => (written, None),
    }
}

fn walk<'a>(mut at: &'a Value, path: &str) -> Option<&'a Value> {
    for written in path.split('.').filter(|step| !step.is_empty()) {
        let (name, place) = step(written);
        at = match at {
            Value::Object(children) => children.get(name)?,
            Value::Array(children) => children.get(name.parse::<usize>().ok()?)?,
            _ => return None,
        };
        if let Some(place) = place {
            at = at.as_array()?.get(place)?;
        }
    }
    Some(at)
}

fn walk_mut<'a>(mut at: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    for written in path.split('.').filter(|step| !step.is_empty()) {
        let (name, place) = step(written);
        at = match at {
            Value::Object(children) => children.get_mut(name)?,
            Value::Array(children) => children.get_mut(name.parse::<usize>().ok()?)?,
            _ => return None,
        };
        if let Some(place) = place {
            at = at.as_array_mut()?.get_mut(place)?;
        }
    }
    Some(at)
}

/// Sets the value at a path, making the objects on the way that are not there,
/// and a list where its first element is asked for. A path that runs into
/// something that holds nothing, such as a number, is left alone.
fn put(mut at: &mut Value, path: &str, value: Value) {
    for written in path.split('.').filter(|step| !step.is_empty()) {
        let (name, place) = step(written);
        if at.is_null() {
            *at = Value::Object(Map::new());
        }
        at = match at {
            Value::Object(children) => children.entry(name).or_insert(Value::Null),
            Value::Array(children) => {
                match name
                    .parse::<usize>()
                    .ok()
                    .and_then(|place| children.get_mut(place))
                {
                    Some(child) => child,
                    None => return,
                }
            }
            _ => return,
        };
        if let Some(place) = place {
            if at.is_null() && place == 0 {
                *at = Value::Array(vec![Value::Null]);
            }
            at = match at
                .as_array_mut()
                .and_then(|children| children.get_mut(place))
            {
                Some(child) => child,
                None => return,
            };
        }
    }
    *at = value;
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{PatchError, patch};
    use crate::{Parser, Replacement};

    fn set(key: &str, value: &str) -> Replacement {
        Replacement {
            key: key.to_owned(),
            value: value.to_owned(),
            only_if: None,
        }
    }

    fn set_if(key: &str, only_if: &str, value: &str) -> Replacement {
        Replacement {
            only_if: Some(only_if.to_owned()),
            ..set(key, value)
        }
    }

    #[test]
    fn replaces_whole_lines_by_how_they_start() {
        let before = "hostname \"old\"\r\nsv_password \"\"\nrcon_password x\n";
        let after = patch(
            Parser::File,
            before,
            &[
                set("hostname", "hostname \"Mine\""),
                set("maxplayers", "maxplayers 8"),
            ],
        );
        // Windows line endings go, as they do through Wings; a key that no line starts with adds none.
        assert_eq!(
            after.unwrap(),
            "hostname \"Mine\"\nsv_password \"\"\nrcon_password x\n"
        );
    }

    #[test]
    fn sets_ini_keys_and_leaves_the_rest_of_the_file_alone() {
        let before = "; made by the game\nname = old\n\n[ServerSettings]\n# how many\nMaxPlayers=10\n\n[Other]\nKey: 1\n";
        let after = patch(
            Parser::Ini,
            before,
            &[
                set("name", "Mine"),
                set("ServerSettings.MaxPlayers", "20"),
                set("ServerSettings.Port", "27015"),
                set("Other.Key", "2"),
                set("[New.Section].Key.With.Dots", "yes"),
            ],
        );
        assert_eq!(
            after.unwrap(),
            "; made by the game\nname = Mine\n\n[ServerSettings]\n# how many\nMaxPlayers=20\nPort=27015\n\n[Other]\nKey: 2\n\n[New.Section]\nKey.With.Dots=yes\n"
        );
        assert_eq!(
            patch(Parser::Ini, "", &[set("a.b", "c")]).unwrap(),
            "[a]\nb=c\n"
        );
    }

    #[test]
    fn sets_json_by_path_and_keeps_the_kinds_of_value() {
        let before = r#"{ "zebra": 1, "server": { "port": 1, "name": "old", "id": "7" }, "listeners": [{ "host": "x" }] }"#;
        let after = patch(
            Parser::Json,
            before,
            &[
                set("server.port", "25565"),
                set("server.name", "Mine"),
                // Text stays text even when a number is put there.
                set("server.id", "8"),
                set("server.query.enabled", "true"),
                set("listeners[0].host", "0.0.0.0:25565"),
                set("rcon[0].port", "25575"),
            ],
        )
        .unwrap();
        let read: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            read,
            json!({
                "zebra": 1,
                "server": { "port": 25565, "name": "Mine", "id": "8", "query": { "enabled": true } },
                "listeners": [{ "host": "0.0.0.0:25565" }],
                "rcon": [{ "port": 25575 }],
            })
        );
        // The file's own order of keys, and four spaces, as Wings indents.
        assert!(after.starts_with("{\n    \"zebra\": 1,\n    \"server\": {"));
        assert!(matches!(
            patch(Parser::Json, "not json", &[]),
            Err(PatchError::Json(_))
        ));
    }

    #[test]
    fn rewrites_every_child_that_matches_a_pattern() {
        // The BungeeCord egg: servers that the proxy would look for on itself
        // are pointed at the host instead, each keeping its own port.
        let before = "servers:\n  lobby:\n    address: localhost:25566\n  survival:\n    address: 127.0.0.1\n  away:\n    address: play.example.com:25565\nlisteners:\n  - query_port: 1\n";
        let after = patch(
            Parser::Yaml,
            before,
            &[
                set_if(
                    "servers.*.address",
                    r"regex:^(127\.0\.0\.1|localhost)(:\d{1,5})?$",
                    "10.213.80.1$2",
                ),
                set("listeners[0].query_port", "25577"),
                set_if("servers.away.address", "somewhere else", "never set"),
                set_if("servers.away.restricted", "false", "true"),
            ],
        )
        .unwrap();
        let read: Value = serde_saphyr::from_str(&after).unwrap();
        assert_eq!(
            read,
            json!({
                "servers": {
                    "lobby": { "address": "10.213.80.1:25566" },
                    "survival": { "address": "10.213.80.1" },
                    "away": { "address": "play.example.com:25565", "restricted": true },
                },
                "listeners": [{ "query_port": 25577 }],
            })
        );
    }

    #[test]
    fn says_what_it_cannot_set_up() {
        assert!(matches!(
            patch(Parser::Xml, "<a/>", &[]),
            Err(PatchError::Xml)
        ));
        let said = patch(Parser::Yaml, "a: [", &[]).unwrap_err().to_string();
        assert!(
            said.starts_with("it is not valid YAML") && !said.contains('\n'),
            "{said}"
        );
        // The properties parser is the one in `properties`, reached the same way.
        assert_eq!(
            patch(Parser::Properties, "a=1\n", &[set("a", "2")]).unwrap(),
            "a=2\n"
        );
    }
}
