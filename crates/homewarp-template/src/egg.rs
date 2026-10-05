//! Import of Pterodactyl (`PTDL_v1`, `PTDL_v2`) and Pelican (`PLCN_v1` to `v3`) eggs.
//!
//! The formats differ mostly in how loosely they were exported: the same field is
//! a list in one, a `|`-joined string in another, and an empty PHP array, which
//! JSON and YAML render as `{}`, in a third. Everything is read as a generic value
//! first and normalised by hand, so that each of those spellings is accepted.

use serde_json::{Map, Value};

use crate::{ConfigFile, Image, Install, Parser, Stop, Template, Variable};

const VERSIONS: [&str; 5] = ["PTDL_v1", "PTDL_v2", "PLCN_v1", "PLCN_v2", "PLCN_v3"];

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not valid YAML: {0}")]
    Yaml(String),
    #[error("unsupported egg format {0:?}")]
    Version(String),
    #[error("`{0}` is missing")]
    Missing(String),
    #[error("`{0}` is not what an egg should have there")]
    Shape(String),
    #[error("{0}")]
    Unsupported(String),
}

/// Reads an egg, as JSON or YAML.
pub fn import(text: &str) -> Result<Template, ImportError> {
    let root: Value = if text.trim_start().starts_with('{') {
        serde_json::from_str(text)?
    } else {
        serde_saphyr::from_str(text).map_err(|error| ImportError::Yaml(error.to_string()))?
    };

    let version = required(&root, &["meta", "version"])?;
    if !VERSIONS.contains(&version.as_str()) {
        return Err(ImportError::Version(version));
    }

    // PLCN_v3 names its startup commands; the first is the default.
    let startup = match at(&root, &["startup_commands"]) {
        Some(Value::Object(commands)) => commands
            .values()
            .next()
            .and_then(scalar)
            .ok_or_else(|| ImportError::Missing("startup_commands".to_owned()))?,
        _ => required(&root, &["startup"])?,
    };

    let started = object(at(&root, &["config", "startup"]), "config.startup")?;
    let done = match started.get("done") {
        Some(Value::String(one)) => vec![one.clone()],
        many => list(many, "config.startup.done")?,
    };

    Ok(Template {
        name: required(&root, &["name"])?,
        description: optional(&root, &["description"]),
        images: images(&root)?,
        startup,
        done,
        stop: stop(&optional(&root, &["config", "stop"])),
        config_files: config_files(&root)?,
        install: install(&root)?,
        variables: variables(&root)?,
        features: list(at(&root, &["features"]), "features")?,
    })
}

fn images(root: &Value) -> Result<Vec<Image>, ImportError> {
    let images: Vec<Image> = match at(root, &["docker_images"]) {
        Some(Value::Object(labelled)) => labelled
            .iter()
            .map(|(label, image)| {
                let image =
                    scalar(image).ok_or_else(|| ImportError::Shape("docker_images".to_owned()))?;
                Ok(Image {
                    label: label.clone(),
                    image,
                })
            })
            .collect::<Result<_, ImportError>>()?,
        // PTDL_v1 lists images without labels, or names just one.
        _ => {
            let mut names = list(at(root, &["images"]), "images")?;
            names.extend(at(root, &["image"]).and_then(scalar));
            names
                .into_iter()
                .map(|image| Image {
                    label: image.clone(),
                    image,
                })
                .collect()
        }
    };
    if images.is_empty() {
        return Err(ImportError::Missing("docker_images".to_owned()));
    }
    Ok(images)
}

fn stop(raw: &str) -> Stop {
    match raw.strip_prefix('^') {
        // `^C` is how eggs spell an interrupt.
        Some(signal) if signal.eq_ignore_ascii_case("c") => Stop::Signal("SIGINT".to_owned()),
        Some(signal) => Stop::Signal(signal.to_ascii_uppercase()),
        None if raw.is_empty() => Stop::Signal("SIGTERM".to_owned()),
        None => Stop::Command(raw.to_owned()),
    }
}

fn config_files(root: &Value) -> Result<Vec<ConfigFile>, ImportError> {
    object(at(root, &["config", "files"]), "config.files")?
        .iter()
        .map(|(path, entry)| {
            let what = format!("config.files.{path}");
            let parser = match entry.get("parser").and_then(Value::as_str) {
                Some("properties") => Parser::Properties,
                Some("yaml" | "yml") => Parser::Yaml,
                Some("json") => Parser::Json,
                Some("ini") => Parser::Ini,
                Some("xml") => Parser::Xml,
                Some("file") => Parser::File,
                _ => return Err(ImportError::Shape(format!("{what}.parser"))),
            };
            let find = object(entry.get("find"), &format!("{what}.find"))?
                .iter()
                .map(|(key, value)| match scalar(value) {
                    Some(value) => Ok((key.clone(), value)),
                    // A table of pattern → replacement, as the BungeeCord egg uses.
                    None => Err(ImportError::Unsupported(format!(
                        "{what}: `{key}` replaces by pattern, which is not supported yet"
                    ))),
                })
                .collect::<Result<_, ImportError>>()?;
            Ok(ConfigFile {
                path: path.clone(),
                parser,
                find,
            })
        })
        .collect()
}

fn install(root: &Value) -> Result<Option<Install>, ImportError> {
    let script = at(root, &["scripts", "installation", "script"]).and_then(scalar);
    let Some(script) = script.filter(|script| !script.trim().is_empty()) else {
        return Ok(None);
    };
    Ok(Some(Install {
        image: required(root, &["scripts", "installation", "container"])?,
        entrypoint: required(root, &["scripts", "installation", "entrypoint"])?,
        // Scripts edited on Windows carry CRLF, which `ash` and `bash` trip over.
        script: script.replace("\r\n", "\n"),
    }))
}

fn variables(root: &Value) -> Result<Vec<Variable>, ImportError> {
    let items: &[Value] = match at(root, &["variables"]) {
        None => &[],
        Some(Value::Array(items)) => items,
        Some(Value::Object(empty)) if empty.is_empty() => &[],
        Some(_) => return Err(ImportError::Shape("variables".to_owned())),
    };
    let mut variables = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let what = |field: &str| format!("variables[{index}].{field}");
        let env = item
            .get("env_variable")
            .and_then(scalar)
            .ok_or_else(|| ImportError::Missing(what("env_variable")))?;
        // It becomes a name in a container's environment, so hold it to what a shell accepts.
        let mut letters = env.chars();
        let named_well = letters
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && letters.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !named_well {
            return Err(ImportError::Shape(what("env_variable")));
        }
        let rules = match item.get("rules") {
            // Older eggs join the rules with `|`, as Laravel accepts.
            Some(Value::String(joined)) => joined
                .split('|')
                .filter(|rule| !rule.is_empty())
                .map(str::to_owned)
                .collect(),
            other => list(other, &what("rules"))?,
        };
        let sort = item.get("sort").and_then(Value::as_i64).unwrap_or(i64::MAX);
        variables.push((
            sort,
            Variable {
                name: item
                    .get("name")
                    .and_then(scalar)
                    .ok_or_else(|| ImportError::Missing(what("name")))?,
                description: item.get("description").and_then(scalar).unwrap_or_default(),
                env,
                default: item
                    .get("default_value")
                    .and_then(scalar)
                    .unwrap_or_default(),
                rules,
                user_viewable: flag(item.get("user_viewable")),
                user_editable: flag(item.get("user_editable")),
            },
        ));
    }
    variables.sort_by_key(|(sort, _)| *sort);
    Ok(variables
        .into_iter()
        .map(|(_, variable)| variable)
        .collect())
}

/// The value at `path`, with `null` counting as absent.
fn at<'a>(root: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(root, |value, key| value.get(key))
        .filter(|value| !value.is_null())
}

/// Scalars as text: YAML eggs leave numbers and booleans unquoted.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn required(root: &Value, path: &[&str]) -> Result<String, ImportError> {
    let value = at(root, path).ok_or_else(|| ImportError::Missing(path.join(".")))?;
    scalar(value).ok_or_else(|| ImportError::Shape(path.join(".")))
}

fn optional(root: &Value, path: &[&str]) -> String {
    at(root, path).and_then(scalar).unwrap_or_default()
}

/// Booleans arrive as `true`, `1` or `"1"`.
fn flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64() != Some(0),
        Some(Value::String(text)) => text == "1" || text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A list of strings, or the `{}` that an empty PHP array is exported as.
fn list(value: Option<&Value>, what: &str) -> Result<Vec<String>, ImportError> {
    let shape = || ImportError::Shape(what.to_owned());
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(empty)) if empty.is_empty() => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| scalar(item).ok_or_else(shape))
            .collect(),
        Some(_) => Err(shape()),
    }
}

/// An object, which Pterodactyl eggs store as a string of JSON.
fn object(value: Option<&Value>, what: &str) -> Result<Map<String, Value>, ImportError> {
    let shape = || ImportError::Shape(what.to_owned());
    match value {
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(Value::Array(empty)) if empty.is_empty() => Ok(Map::new()),
        Some(Value::String(json)) if json.trim().is_empty() => Ok(Map::new()),
        Some(Value::String(json)) => match serde_json::from_str(json)? {
            Value::Object(map) => Ok(map),
            Value::Array(empty) if empty.is_empty() => Ok(Map::new()),
            _ => Err(shape()),
        },
        Some(_) => Err(shape()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a Pelican panel export, quirks included: `{  }` where a list
    /// is empty, a script with CRLF line endings, variables out of display order.
    const PELICAN: &str = r##"
_comment: 'DO NOT EDIT: FILE GENERATED AUTOMATICALLY BY PANEL'
meta:
  version: PLCN_v3
  update_url: null
exported_at: '2026-04-21T18:27:54+00:00'
name: Example
author: someone@example.com
description: 'An example server.'
features:
  - eula
docker_images:
  'Java 25': 'example.invalid/java:25'
  'Java 21': 'example.invalid/java:21'
file_denylist: {  }
startup_commands:
  Default: 'java -Xms128M -jar {{SERVER_JARFILE}}'
config:
  files:
    server.properties:
      parser: properties
      find:
        server-ip: ''
        server-port: '{{server.allocations.default.port}}'
        max-players: 20
  startup:
    done: ')! For help, type '
  logs: {  }
  stop: stop
scripts:
  installation:
    script: "#!/bin/ash\r\necho hi\r\n"
    container: 'example.invalid/installer:alpine'
    entrypoint: ash
variables:
  -
    sort: 2
    name: 'Server Jar File'
    description: 'The jar to run.'
    env_variable: SERVER_JARFILE
    default_value: server.jar
    user_viewable: true
    user_editable: true
    rules:
      - required
      - 'regex:/^([\w\d._-]+)(\.jar)$/'
  -
    sort: 1
    name: Project
    description: ''
    env_variable: PROJECT
    default_value: paper
    user_viewable: false
    user_editable: false
    rules: {  }
"##;

    /// Shaped like a Pterodactyl export: `config` values are strings of JSON,
    /// rules are joined with `|`, flags are numbers.
    const PTERODACTYL: &str = r#"{
  "meta": { "version": "PTDL_v2", "update_url": null },
  "name": "Example",
  "description": null,
  "features": null,
  "docker_images": { "Debian": "example.invalid/debian:latest" },
  "startup": "./server -port {{SERVER_PORT}}",
  "config": {
    "files": "{\r\n  \"server.cfg\": {\r\n    \"parser\": \"file\",\r\n    \"find\": { \"port\": \"port {{server.build.default.port}}\" }\r\n  }\r\n}",
    "startup": "{\r\n  \"done\": [\"Ready\", \"Listening\"]\r\n}",
    "logs": "{}",
    "stop": "^C"
  },
  "scripts": { "installation": { "script": null, "container": "alpine:3.4", "entrypoint": "ash" } },
  "variables": [
    {
      "name": "Max Players",
      "description": "",
      "env_variable": "MAX_PLAYERS",
      "default_value": 20,
      "user_viewable": 1,
      "user_editable": 0,
      "rules": "required|integer|between:1,100"
    }
  ]
}"#;

    #[test]
    fn imports_a_pelican_egg() {
        let template = import(PELICAN).unwrap();

        assert_eq!(template.name, "Example");
        assert_eq!(
            template.images[0],
            Image {
                label: "Java 25".to_owned(),
                image: "example.invalid/java:25".to_owned()
            }
        );
        assert_eq!(template.images.len(), 2);
        assert_eq!(template.startup, "java -Xms128M -jar {{SERVER_JARFILE}}");
        assert_eq!(template.done, [")! For help, type "]);
        assert_eq!(template.stop, Stop::Command("stop".to_owned()));
        assert_eq!(template.features, ["eula"]);

        let file = &template.config_files[0];
        assert_eq!(
            (file.path.as_str(), file.parser),
            ("server.properties", Parser::Properties)
        );
        assert_eq!(
            file.find,
            [
                ("server-ip".to_owned(), String::new()),
                (
                    "server-port".to_owned(),
                    "{{server.allocations.default.port}}".to_owned()
                ),
                ("max-players".to_owned(), "20".to_owned()),
            ]
        );

        let install = template.install.unwrap();
        assert_eq!(install.script, "#!/bin/ash\necho hi\n");
        assert_eq!(
            (install.image.as_str(), install.entrypoint.as_str()),
            ("example.invalid/installer:alpine", "ash")
        );

        let envs: Vec<&str> = template
            .variables
            .iter()
            .map(|variable| variable.env.as_str())
            .collect();
        assert_eq!(envs, ["PROJECT", "SERVER_JARFILE"]);
        assert!(template.variables[0].rules.is_empty());
        assert_eq!(
            template.variables[1].rules,
            ["required", r"regex:/^([\w\d._-]+)(\.jar)$/"]
        );
        assert!(template.variables[1].user_editable);
    }

    #[test]
    fn imports_a_pterodactyl_egg() {
        let template = import(PTERODACTYL).unwrap();

        assert_eq!(template.startup, "./server -port {{SERVER_PORT}}");
        assert_eq!(template.done, ["Ready", "Listening"]);
        assert_eq!(template.stop, Stop::Signal("SIGINT".to_owned()));
        assert_eq!(template.install, None);
        assert!(template.features.is_empty());
        assert_eq!(template.config_files[0].parser, Parser::File);
        assert_eq!(
            template.config_files[0].find,
            [(
                "port".to_owned(),
                "port {{server.build.default.port}}".to_owned()
            )]
        );

        let players = &template.variables[0];
        assert_eq!(players.default, "20");
        assert_eq!(players.rules, ["required", "integer", "between:1,100"]);
        assert!(players.user_viewable && !players.user_editable);
    }

    #[test]
    fn refuses_what_it_cannot_run() {
        let unknown = PELICAN.replace("PLCN_v3", "PLCN_v9");
        assert!(
            matches!(import(&unknown), Err(ImportError::Version(version)) if version == "PLCN_v9")
        );

        let no_images =
            PTERODACTYL.replace(r#"{ "Debian": "example.invalid/debian:latest" }"#, "{}");
        assert!(
            matches!(import(&no_images), Err(ImportError::Missing(what)) if what == "docker_images")
        );

        let hostile = PTERODACTYL.replace("MAX_PLAYERS", "MAX=1 PLAYERS");
        assert!(
            matches!(import(&hostile), Err(ImportError::Shape(what)) if what == "variables[0].env_variable")
        );

        let by_pattern = PELICAN.replace("server-ip: ''", "server-ip: { 'regex:^127': '0.0.0.0' }");
        assert!(matches!(
            import(&by_pattern),
            Err(ImportError::Unsupported(_))
        ));
    }
}
