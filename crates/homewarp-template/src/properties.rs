//! The `properties` config-file parser: Java `.properties`, as in Minecraft's
//! `server.properties`.

/// Sets each key to its value and leaves every other line (comments, blank
/// lines, keys it was not asked about) as it was. A key the file does not have
/// yet is appended.
///
/// Keys are matched as written; escaped separators inside a key and values
/// continued over several lines are not understood.
pub fn patch(text: &str, pairs: &[(String, String)]) -> String {
    let mut written = vec![false; pairs.len()];
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        let found = key_of(line).and_then(|key| pairs.iter().position(|(wanted, _)| wanted == key));
        match found {
            // Every occurrence is rewritten: Java keeps the last one it reads.
            Some(index) => {
                push(&mut out, &pairs[index]);
                written[index] = true;
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (pair, _) in pairs.iter().zip(&written).filter(|(_, written)| !**written) {
        push(&mut out, pair);
    }
    out
}

/// The key of a `key=value`, `key:value` or `key value` line; `None` for a
/// comment or a blank line.
fn key_of(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with(['#', '!']) {
        return None;
    }
    let end = line
        .find(|c: char| c == '=' || c == ':' || c.is_whitespace())
        .unwrap_or(line.len());
    Some(&line[..end])
}

fn push(out: &mut String, (key, value): &(String, String)) {
    out.push_str(key);
    out.push('=');
    // Java would drop a leading space and read a backslash as an escape.
    if value.starts_with(' ') {
        out.push('\\');
    }
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::patch;

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn rewrites_the_keys_it_is_given_and_nothing_else() {
        let before = "#Minecraft server properties\nmotd=A Minecraft Server\nserver-port=25565\nserver-ip=10.0.0.1\n";
        let after = patch(
            before,
            &pairs(&[("server-ip", ""), ("server-port", "25600")]),
        );
        assert_eq!(
            after,
            "#Minecraft server properties\nmotd=A Minecraft Server\nserver-port=25600\nserver-ip=\n"
        );
    }

    #[test]
    fn appends_keys_the_file_does_not_have() {
        let after = patch("motd=hi\r\n", &pairs(&[("query.port", "25600")]));
        assert_eq!(after, "motd=hi\nquery.port=25600\n");
        assert_eq!(patch("", &pairs(&[("a", "1")])), "a=1\n");
    }

    #[test]
    fn understands_the_other_ways_to_write_a_pair() {
        let before = "  server-port = 1\nquery.port:2\nlevel-name world\n#server-port=3\nserver-port-extra=4\n";
        let after = patch(
            before,
            &pairs(&[
                ("server-port", "9"),
                ("query.port", "9"),
                ("level-name", "lobby"),
            ]),
        );
        assert_eq!(
            after,
            "server-port=9\nquery.port=9\nlevel-name=lobby\n#server-port=3\nserver-port-extra=4\n"
        );
    }

    #[test]
    fn rewrites_every_duplicate_and_escapes_values() {
        let after = patch(
            "port=1\nport=2\n",
            &pairs(&[("port", "3"), ("path", " C:\\games\nx")]),
        );
        assert_eq!(after, "port=3\nport=3\npath=\\ C:\\\\games\\nx\n");
    }
}
