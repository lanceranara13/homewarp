//! Eggs and config files that nobody would write (PLAN.md §11, Phase 5).
//!
//! An egg is text from somewhere on the internet, and a config file is a
//! server's own, which the game or anyone let into its files can make whatever
//! they like. Core reads both, and reads the second as root, every time the
//! server starts. So neither may bring it down: not by a panic, not by a
//! reading that never ends, not by one that takes all the memory there is.
//!
//! What is here is not clever. A few real eggs and files are changed at random,
//! thousands of times, and each result is read. A reading that panics or takes
//! longer than a person would wait fails the test with what was read. The
//! count is small enough to run with every other test; `HOMEWARP_FUZZ=<count>`
//! runs as many as are asked for, and `HOMEWARP_FUZZ_SEED=<number>` starts
//! somewhere else.

use std::{
    panic::{self, AssertUnwindSafe},
    time::{Duration, Instant},
};

use homewarp_template::{Parser, Replacement, config, import, properties, rules, substitute};

/// How many are tried when nothing says otherwise.
const TRIES: u64 = 4000;
/// Longer than this for one reading is a reading that could be made endless.
const TOO_LONG: Duration = Duration::from_secs(4);
/// As long as anything tried gets. An egg is a few thousand bytes.
const LONGEST: usize = 1 << 16;

const PELICAN: &str = r##"
meta:
  version: PLCN_v3
name: Example
description: 'An example server.'
features:
  - eula
docker_images:
  'Java 21': 'example.invalid/java:21'
  'Java 17': 'example.invalid/java:17'
startup_commands:
  Default: 'java -Xms128M -jar {{SERVER_JARFILE}}'
config:
  files:
    server.properties:
      parser: properties
      find:
        server-ip: ''
        server-port: '{{server.allocations.default.port}}'
    config.yml:
      parser: yaml
      find:
        'listeners[0].query_port': '{{server.allocations.default.port}}'
        'servers.*.address':
          'regex:^(127\.0\.0\.1|localhost)(:\d{1,5})?$': '{{config.docker.interface}}$2'
  startup:
    done: ')! For help, type '
  stop: stop
scripts:
  installation:
    script: "#!/bin/ash\necho hi\n"
    container: 'example.invalid/installer:alpine'
    entrypoint: ash
variables:
  -
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
    name: 'Build'
    description: ''
    env_variable: BUILD_NUMBER
    default_value: latest
    user_viewable: true
    user_editable: true
    rules:
      - required
      - string
      - 'max:20'
"##;

const PTERODACTYL: &str = r#"{
  "meta": { "version": "PTDL_v2", "update_url": null },
  "name": "Example",
  "description": null,
  "features": null,
  "docker_images": { "Debian": "example.invalid/debian:latest" },
  "startup": "./server -port {{SERVER_PORT}}",
  "config": {
    "files": "{\r\n  \"server.cfg\": {\r\n    \"parser\": \"file\",\r\n    \"find\": { \"port\": \"port {{server.build.default.port}}\" }\r\n  },\r\n \"settings.json\": { \"parser\": \"json\", \"find\": { \"net.port\": \"{{server.build.default.port}}\" } }\r\n}",
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

/// A server's own files, one for each way Homewarp reads them.
const FILES: [(Parser, &str); 6] = [
    (
        Parser::Properties,
        "#Minecraft server properties\nserver-port=25565\nmotd=A \\u00a7lserver\\: hello\nlevel-name = world\n!comment\nempty=\n",
    ),
    (
        Parser::Yaml,
        "listeners:\n  - query_port: 25577\n    host: 0.0.0.0:25577\nservers:\n  lobby:\n    address: localhost:25566\n    restricted: false\ngroups: &groups\n  admin: [a, b]\nother: *groups\n",
    ),
    (
        Parser::Json,
        "{\n  \"net\": { \"port\": 7777, \"hosts\": [\"a\", \"b\"] },\n  \"name\": \"x\",\n  \"deep\": [[[[1]]]],\n  \"n\": 1.5e3\n}\n",
    ),
    (
        Parser::Ini,
        "; a comment\n[ServerSettings]\nPort=7777\nName = \"My server\"\n\n[/Script/Engine.GameSession]\nMaxPlayers=70\n",
    ),
    (
        Parser::File,
        "port 27015\nhostname \"A server\"\nsv_lan 0\n// comment\n",
    ),
    (Parser::Xml, "<config><port>1</port></config>\n"),
];

/// What a file is asked to be set to, the ordinary way and with a pattern.
const FINDS: [(&str, &str, Option<&str>); 9] = [
    ("server-port", "{{server.build.default.port}}", None),
    ("listeners[0].query_port", "25565", None),
    (
        "servers.*.address",
        "{{config.docker.interface}}$2",
        Some("regex:^(127\\.0\\.0\\.1|localhost)(:\\d{1,5})?$"),
    ),
    ("net.port", "1", None),
    ("ServerSettings.Port", "2", None),
    ("port", "port 3", None),
    ("a.b.c.d.e", "deep", None),
    ("", "", Some("")),
    ("name", "y", Some("x")),
];

/// Laravel's rules, as eggs write them.
const RULES: [&str; 12] = [
    "required",
    "nullable",
    "string",
    "integer",
    "numeric",
    "boolean",
    "max:20",
    "between:1,100",
    "in:a,b,c",
    "regex:/^([\\w\\d._-]+)(\\.jar)$/",
    "alpha_dash",
    "digits_between:1,5",
];

/// What is put in where it does not belong: the marks each format gives a
/// meaning to, and the things a reader is tempted to trust.
const SPLINTERS: [&str; 40] = [
    "{{",
    "}}",
    "{",
    "}",
    "[",
    "]",
    "\"",
    "'",
    ":",
    ": ",
    "- ",
    "\n",
    "\r\n",
    "\t",
    "  ",
    "#",
    ";",
    "=",
    "\\",
    "\\u",
    "\0",
    "&a ",
    "*a",
    "<<: *a",
    "!!binary ",
    "|",
    ">",
    "---",
    "...",
    "regex:",
    "regex:/(a*)*b/",
    "$1",
    "$99999999999",
    ".*",
    "[0]",
    "[99999999999999999999]",
    "*",
    ".",
    "99999999999999999999999999",
    "\u{feff}",
];

/// Numbers that look like chance and come the same every time: a failure can
/// be had again by its seed.
struct Chance(u64);

impl Chance {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, most: usize) -> usize {
        match most {
            0 => 0,
            most => (self.next() % most as u64) as usize,
        }
    }

    /// A number below `most` that is small more often than not.
    fn few(&mut self, most: usize) -> usize {
        let most = 1 + self.below(most);
        self.below(most)
    }

    fn of<'a, T>(&mut self, all: &'a [T]) -> &'a T {
        &all[self.below(all.len())]
    }
}

/// The nearest place at or before `at` where a character begins.
fn whole(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// A piece of `text`, cut where characters begin.
fn piece<'a>(text: &'a str, chance: &mut Chance) -> &'a str {
    let from = whole(text, chance.below(text.len() + 1));
    let length = chance.few(64);
    &text[from..whole(text, from + length)]
}

/// `text` with a few things done to it that a person would not do.
fn changed(text: &str, chance: &mut Chance) -> String {
    let mut text = text.to_owned();
    for _ in 0..1 + chance.below(6) {
        let at = whole(&text, chance.below(text.len() + 1));
        match chance.below(9) {
            // Something put in that means something to one of the formats.
            0 | 1 => text.insert_str(at, chance.of(&SPLINTERS)),
            // A piece taken out.
            2 => {
                let until = whole(&text, at + chance.few(80));
                text.replace_range(at..until, "");
            }
            // A piece said again, a few times or many.
            3 => {
                let again = piece(&text, chance).repeat(1 + chance.few(40));
                text.insert_str(at, &again);
            }
            // Cut off where it stands.
            4 => text.truncate(at),
            // One character made another.
            5 => {
                let until = whole(&text, at + 1).max(at);
                let other = char::from_u32(chance.below(0x250) as u32).unwrap_or('?');
                if until > at {
                    text.replace_range(at..until, other.encode_utf8(&mut [0; 4]));
                }
            }
            // Nested, as deep as chance has it.
            6 => {
                let depth = 1 + chance.few(600);
                let (open, shut) =
                    *chance.of(&[("[", "]"), ("{\"a\":", "}"), ("- ", ""), ("a:\n ", "")]);
                text.insert_str(
                    at,
                    &format!("{}1{}", open.repeat(depth), shut.repeat(depth)),
                );
            }
            // A piece of one of the other things that are read.
            7 => {
                let other = *chance.of(&[PELICAN, PTERODACTYL, FILES[1].1, FILES[2].1]);
                let taken = piece(other, chance).to_owned();
                text.insert_str(at, &taken);
            }
            // A number, long.
            _ => text.insert_str(at, &"9".repeat(1 + chance.below(400))),
        }
        if text.len() > LONGEST {
            text.truncate(whole(&text, LONGEST));
        }
    }
    text
}

/// Reads something, and fails with what it was if the reading panics or goes
/// on too long.
fn read<T>(what: &str, input: &[&str], reading: impl FnOnce() -> T) -> Option<T> {
    let began = Instant::now();
    let read = panic::catch_unwind(AssertUnwindSafe(reading));
    let took = began.elapsed();
    let shown = || {
        input
            .iter()
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join("\n  with ")
    };
    assert!(read.is_ok(), "{what} panicked on:\n  {}", shown());
    assert!(took < TOO_LONG, "{what} took {took:?} on:\n  {}", shown());
    read.ok()
}

/// One egg, read every way an egg is: imported, and then used as a server's
/// template is used. True if it was still an egg.
fn an_egg(text: &str) -> bool {
    let Some(Ok(template)) = read("import", &[text], || import(text)) else {
        return false;
    };
    // What was read is kept as JSON, and read from that again at every start.
    let kept = serde_json::to_string(&template).expect("a template is plain data");
    let again: homewarp_template::Template =
        serde_json::from_str(&kept).expect("what was kept can be read");
    assert_eq!(again, template);
    read("substitute", &[&template.startup], || {
        substitute(&template.startup, |name| Some(name.to_uppercase()))
    });
    for variable in &template.variables {
        let rules: Vec<&str> = variable.rules.iter().map(String::as_str).collect();
        read("rules", &rules, || rules::unsupported(&variable.rules));
        for value in ["", "20", "server.jar", &variable.default] {
            read("a rule", &[&rules.join(" | "), value], || {
                rules::check(&variable.rules, value)
            });
        }
    }
    for file in &template.config_files {
        for (_, sample) in FILES {
            let finds = format!("{:?}", file.find);
            read("patch, by an egg", &[&finds, sample], || {
                config::patch(file.parser, sample, &file.find)
            });
        }
    }
    true
}

/// One file of a server's, set as an egg would have it set.
fn a_file(parser: Parser, text: &str, finds: &[Replacement]) {
    let shown = format!("{finds:?}");
    read("patch", &[&format!("{parser:?}"), text, &shown], || {
        config::patch(parser, text, finds)
    });
}

fn seed() -> u64 {
    let asked = std::env::var("HOMEWARP_FUZZ_SEED").ok();
    asked
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(0x4857_2026)
}

fn tries() -> u64 {
    let asked = std::env::var("HOMEWARP_FUZZ").ok();
    asked.and_then(|tries| tries.parse().ok()).unwrap_or(TRIES)
}

#[test]
fn the_seeds_are_what_they_are_meant_to_be() {
    // Changed from something that reads, or nothing past the first line is tried.
    assert!(import(PELICAN).is_ok(), "{:?}", import(PELICAN).err());
    assert!(
        import(PTERODACTYL).is_ok(),
        "{:?}",
        import(PTERODACTYL).err()
    );
    for (parser, text) in FILES {
        let set = [Replacement {
            key: "a".to_owned(),
            value: "b".to_owned(),
            only_if: None,
        }];
        assert_eq!(
            config::patch(parser, text, &set).is_ok(),
            parser != Parser::Xml,
            "{parser:?}"
        );
    }
}

#[test]
fn no_egg_brings_the_importer_down() {
    let mut chance = Chance(seed() | 1);
    let (tries, mut still_eggs) = (tries(), 0);
    for _ in 0..tries {
        let seed = *chance.of(&[PELICAN, PTERODACTYL]);
        still_eggs += u64::from(an_egg(&changed(seed, &mut chance)));
    }
    // Changed too far, nothing reads past the first line, and what is done
    // with an egg once it is read would go untried. Both kinds are wanted:
    // those that are refused, and those that are taken and are odd.
    println!("{still_eggs} of {tries} changed eggs were still eggs");
    assert!(
        still_eggs * 20 > tries && still_eggs * 10 < tries * 9,
        "{still_eggs} of {tries} changed eggs were still eggs"
    );
}

#[test]
fn no_file_of_a_servers_brings_the_patcher_down() {
    let mut chance = Chance(seed().rotate_left(17) | 1);
    for _ in 0..tries() {
        let (parser, text) = *chance.of(&FILES);
        // Its own kind of file changed, or another kind's read as its own.
        let text = match chance.below(8) {
            0 => chance.of(&FILES).1.to_owned(),
            _ => changed(text, &mut chance),
        };
        let finds: Vec<Replacement> = (0..1 + chance.below(4))
            .map(|_| {
                let (key, value, only_if) = *chance.of(&FINDS);
                let mut part = |text: &str| match chance.below(3) {
                    0 => changed(text, &mut chance),
                    _ => text.to_owned(),
                };
                Replacement {
                    key: part(key),
                    value: part(value),
                    only_if: only_if.map(&mut part),
                }
            })
            .collect();
        a_file(parser, &text, &finds);
    }
}

#[test]
fn no_rule_or_value_brings_the_checker_down() {
    let mut chance = Chance(seed().rotate_left(31) | 1);
    for _ in 0..tries() {
        let rules: Vec<String> = (0..1 + chance.below(4))
            .map(|_| {
                let rule = *chance.of(&RULES);
                match chance.below(2) {
                    0 => changed(rule, &mut chance),
                    _ => rule.to_owned(),
                }
            })
            .collect();
        let value = changed("server-1.jar", &mut chance);
        let shown: Vec<&str> = rules.iter().map(String::as_str).collect();
        read("rules", &shown, || rules::unsupported(&rules));
        read("a rule", &[&shown.join(" | "), &value], || {
            rules::check(&rules, &value)
        });
        let text = changed(FILES[0].1, &mut chance);
        let pairs = [(changed("server-port", &mut chance), value.clone())];
        read("properties", &[&text, &pairs[0].0, &value], || {
            properties::patch(&text, &pairs)
        });
        read("substitute", &[&text], || {
            substitute(&text, |_| Some(value.clone()))
        });
    }
}

/// A few lines of YAML that say a great deal: each name stands for ten of the
/// one before it. Read in full it is ten thousand million things.
fn bomb() -> String {
    let mut yaml = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..=10 {
        let before = format!("*a{}", level - 1);
        yaml += &format!(
            "a{level}: &a{level} [{}]\n",
            [before.as_str(); 10].join(", ")
        );
    }
    yaml
}

#[test]
fn a_file_that_says_more_than_it_holds_is_not_taken_at_its_word() {
    let bomb = bomb();
    let set = [Replacement {
        key: "a10".to_owned(),
        value: "1".to_owned(),
        only_if: None,
    }];
    // As a server's config file, read at every start.
    read("patch, of a bomb", &[&bomb], || {
        let _ = config::patch(Parser::Yaml, &bomb, &set);
    });
    // And as an egg, or in the middle of one.
    read("import, of a bomb", &[&bomb], || {
        let _ = import(&bomb);
    });
    let egg = format!("{bomb}{PELICAN}");
    read("import, of an egg with a bomb", &[&egg], || {
        let _ = import(&egg);
    });
    // And the same thing done by depth: a thing in a thing, a hundred thousand deep.
    for (open, shut) in [("[", "]"), ("{\"a\":", "}")] {
        let deep = format!("{}1{}", open.repeat(100_000), shut.repeat(100_000));
        read("patch, of a deep file", &[open], || {
            let _ = config::patch(Parser::Json, &deep, &set);
            let _ = config::patch(Parser::Yaml, &deep, &set);
        });
        read("import, of a deep egg", &[open], || {
            let _ = import(&deep);
        });
    }
}
