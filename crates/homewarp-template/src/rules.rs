//! Laravel's validation rules, as far as eggs use them (PLAN.md §5.6 and §10).
//!
//! An egg gives each variable a list such as `required`, `string`, `max:20`.
//! The panels hand those to Laravel; this is the part of Laravel that the eggs
//! in the corpus turn out to need, written to give the same answer for the
//! values a person types into a form, which are always text.

use regex_lite::{Regex, RegexBuilder};

enum Rule<'a> {
    Required,
    /// `nullable` and `string` ask nothing of a value that is already text.
    Nothing,
    Numeric,
    Integer,
    Boolean,
    Min(f64),
    Max(f64),
    Between(f64, f64),
    Size(f64),
    GreaterThan(f64),
    In(Vec<&'a str>),
    NotIn(Vec<&'a str>),
    EndsWith(Vec<&'a str>),
    Regex(Regex),
    AlphaDash,
    AlphaNum,
    DigitsBetween(usize, usize),
    Url,
}

/// Why this list of rules cannot be followed, if it cannot: a rule this module
/// does not know, or one written in a way it cannot read. Asked when an egg is
/// imported, so that the answer does not wait for a server to be made.
pub fn unsupported(rules: &[String]) -> Option<String> {
    rules.iter().find_map(|rule| parse(rule).err())
}

/// Checks a value against a variable's rules. The error finishes the sentence
/// that starts with the variable's name: "must be a whole number".
pub fn check(rules: &[String], value: &str) -> Result<(), String> {
    let rules: Vec<Rule> = rules
        .iter()
        .map(|rule| parse(rule))
        .collect::<Result<_, _>>()?;
    // Laravel runs nothing but `required` on an empty field.
    if value.is_empty() {
        return match rules.iter().any(|rule| matches!(rule, Rule::Required)) {
            true => Err("is required".to_owned()),
            false => Ok(()),
        };
    }
    // It measures a number by its value and anything else by its length.
    let number = value.trim().parse::<f64>().ok().filter(|n| n.is_finite());
    let numeric = rules
        .iter()
        .any(|rule| matches!(rule, Rule::Numeric | Rule::Integer));
    let length = value.chars().count();
    let size = number.filter(|_| numeric).unwrap_or(length as f64);
    let unit = if numeric { "" } else { " characters" };

    for rule in &rules {
        let broken = match rule {
            Rule::Required | Rule::Nothing => None,
            Rule::Numeric => number.is_none().then(|| "must be a number".to_owned()),
            Rule::Integer => value
                .trim()
                .parse::<i64>()
                .is_err()
                .then(|| "must be a whole number".to_owned()),
            Rule::Boolean => (!matches!(value, "0" | "1" | "true" | "false"))
                .then(|| "must be 0 or 1".to_owned()),
            Rule::Min(least) => (size < *least).then(|| format!("must be at least {least}{unit}")),
            Rule::Max(most) => (size > *most).then(|| format!("must be at most {most}{unit}")),
            Rule::Between(least, most) => (size < *least || size > *most)
                .then(|| format!("must be between {least} and {most}{unit}")),
            Rule::Size(exact) => (size != *exact).then(|| format!("must be {exact}{unit}")),
            Rule::GreaterThan(floor) => {
                (size <= *floor).then(|| format!("must be more than {floor}{unit}"))
            }
            Rule::In(allowed) => (!allowed.contains(&value))
                .then(|| format!("must be one of {}", allowed.join(", "))),
            Rule::NotIn(refused) => refused
                .contains(&value)
                .then(|| format!("must not be {value}")),
            Rule::EndsWith(endings) => (!endings.iter().any(|ending| value.ends_with(ending)))
                .then(|| format!("must end with {}", endings.join(" or "))),
            Rule::Regex(pattern) => (!pattern.is_match(value))
                .then(|| "is not written the way this template asks".to_owned()),
            Rule::AlphaDash => (!value
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_')))
            .then(|| "may hold only letters, digits, dashes and underscores".to_owned()),
            Rule::AlphaNum => (!value.chars().all(char::is_alphanumeric))
                .then(|| "may hold only letters and digits".to_owned()),
            Rule::DigitsBetween(least, most) => (!value.bytes().all(|b| b.is_ascii_digit())
                || !(*least..=*most).contains(&length))
            .then(|| format!("must be {least} to {most} digits")),
            Rule::Url => (!is_url(value)).then(|| "must be a web address".to_owned()),
        };
        if let Some(reason) = broken {
            return Err(reason);
        }
    }
    Ok(())
}

fn parse(rule: &str) -> Result<Rule<'_>, String> {
    let (name, given) = rule.split_once(':').unwrap_or((rule, ""));
    let odd = || format!("`{rule}` is not a rule Homewarp can read");
    let number = |text: &str| text.trim().parse::<f64>().map_err(|_| odd());
    let list = || -> Vec<&str> {
        given
            .split(',')
            .map(|item| item.trim_matches('"'))
            .collect()
    };
    let pair = || given.split_once(',').ok_or_else(odd);
    Ok(match name {
        "required" => Rule::Required,
        "nullable" | "string" => Rule::Nothing,
        "numeric" => Rule::Numeric,
        // One egg in the corpus writes `int`.
        "integer" | "int" => Rule::Integer,
        "boolean" => Rule::Boolean,
        "min" => Rule::Min(number(given)?),
        "max" => Rule::Max(number(given)?),
        "size" => Rule::Size(number(given)?),
        "gt" => Rule::GreaterThan(number(given)?),
        "between" => {
            let (least, most) = pair()?;
            Rule::Between(number(least)?, number(most)?)
        }
        "digits_between" => {
            let (least, most) = pair()?;
            let digits = |text: &str| text.trim().parse::<usize>().map_err(|_| odd());
            Rule::DigitsBetween(digits(least)?, digits(most)?)
        }
        "in" => Rule::In(list()),
        "not_in" => Rule::NotIn(list()),
        "ends_with" => Rule::EndsWith(list()),
        "regex" => Rule::Regex(pattern(given).ok_or_else(odd)?),
        "alpha_dash" => Rule::AlphaDash,
        "alpha_num" => Rule::AlphaNum,
        "url" => Rule::Url,
        _ => return Err(format!("`{rule}` is not a rule Homewarp knows")),
    })
}

/// A pattern as PHP writes one: between two delimiters, with flags after the
/// second. `None` if it is not that, or uses what this engine does not have.
fn pattern(written: &str) -> Option<Regex> {
    let open = written.chars().next()?;
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        c if c.is_alphanumeric() || c == '\\' || c.is_whitespace() => return None,
        c => c,
    };
    let end = written.rfind(close).filter(|end| *end > 0)?;
    let mut builder = RegexBuilder::new(&written[open.len_utf8()..end]);
    for flag in written[end + close.len_utf8()..].chars() {
        match flag {
            'i' => builder.case_insensitive(true),
            'm' => builder.multi_line(true),
            's' => builder.dot_matches_new_line(true),
            'x' => builder.ignore_whitespace(true),
            // Text here is always Unicode.
            'u' => &mut builder,
            _ => return None,
        };
    }
    builder.build().ok()
}

fn is_url(value: &str) -> bool {
    value.split_once("://").is_some_and(|(scheme, rest)| {
        scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            && !rest.is_empty()
            && !rest.contains(char::is_whitespace)
    })
}

#[cfg(test)]
mod tests {
    use super::{check, unsupported};

    fn rules(written: &str) -> Vec<String> {
        written.split('|').map(str::to_owned).collect()
    }

    /// Whether `value` passes the rules written Laravel's way, joined by `|`.
    fn passes(written: &str, value: &str) -> bool {
        check(&rules(written), value).is_ok()
    }

    #[test]
    fn an_empty_value_fails_only_what_is_required() {
        assert!(passes("nullable|string|max:20", ""));
        assert!(passes("integer|between:1,5", ""));
        assert_eq!(
            check(&rules("required|string"), "").unwrap_err(),
            "is required"
        );
    }

    #[test]
    fn measures_numbers_by_value_and_text_by_length() {
        assert!(passes("required|string|max:20", "latest"));
        assert!(!passes("required|string|max:5", "latest"));
        assert!(passes("required|integer|between:1,100", "100"));
        assert!(!passes("required|integer|between:1,100", "101"));
        // Without a rule that says number, `between` counts characters.
        assert!(passes("required|between:1,3", "101"));
        assert!(passes("required|numeric|min:0.5", "0.5"));
        assert!(!passes("required|numeric|gt:0", "0"));
        assert!(passes("required|string|size:32", &"a".repeat(32)));
        assert_eq!(
            check(&rules("required|string|max:5"), "latest").unwrap_err(),
            "must be at most 5 characters"
        );
        assert_eq!(
            check(&rules("required|integer|max:5"), "6").unwrap_err(),
            "must be at most 5"
        );
    }

    #[test]
    fn knows_the_kinds_of_value_eggs_ask_for() {
        assert!(passes("required|integer", "-25565") && passes("required|int", "7"));
        assert!(!passes("required|integer", "1.5") && !passes("required|integer", "port"));
        assert!(passes("required|numeric", "1.5") && !passes("required|numeric", "NaN"));
        assert!(passes("required|boolean", "0") && passes("required|boolean", "true"));
        assert!(!passes("required|boolean", "yes"));
        assert!(passes("required|alpha_dash", "my-world_2") && !passes("alpha_dash", "my world"));
        assert!(passes("alpha_num", "abc123") && !passes("alpha_num", "abc-123"));
        assert!(passes("digits_between:1,5", "27015") && !passes("digits_between:1,5", "270150"));
        assert!(!passes("digits_between:1,5", "27o15"));
        assert!(passes("url", "https://example.com/pack.zip") && !passes("url", "example.com"));
        assert!(passes("required|string|in:true,false", "false"));
        assert!(!passes("required|string|in:true,false", "maybe"));
        assert!(passes("not_in:0", "1") && !passes("not_in:0", "0"));
        assert!(passes("ends_with:.jar,.zip", "server.jar") && !passes("ends_with:.jar", "server"));
    }

    #[test]
    fn follows_patterns_written_for_php() {
        let jar = r"required|regex:/^([\w\d._-]+)(\.jar)$/";
        assert!(passes(jar, "server.jar") && !passes(jar, "server.exe"));
        assert!(!passes(jar, "../server.jar"));
        assert!(passes(r"regex:/^[a-z]+$/i", "Paper"));
        assert!(passes(r"regex:#^\d+/\d+$#", "1/2"));
    }

    #[test]
    fn says_which_rule_it_cannot_follow() {
        assert_eq!(unsupported(&rules("required|string|max:20")), None);
        let unknown = unsupported(&rules("required|prohibited_if:x,1")).unwrap();
        assert!(unknown.contains("prohibited_if:x,1") && unknown.contains("knows"));
        for unreadable in ["max:lots", "between:1", "regex:^unfenced$", "regex:/(?=x)/"] {
            let said = unsupported(&rules(unreadable)).unwrap();
            assert!(said.contains(unreadable), "{said}");
        }
    }
}
