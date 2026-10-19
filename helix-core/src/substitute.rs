//! Vim-style `:s/pattern/replacement/flags`.
//!
//! The pattern uses Helix's regex syntax (the `regex` crate) plus Vim's `\<`
//! and `\>` word boundaries. The replacement understands:
//!
//! - `\1` .. `\9`, `$1`, `${1}`, `${name}`, `$name`: capture groups
//! - `&`, `\0`, `$0`: the whole match
//! - `\n`, `\t`, `\r`: newline, tab, carriage return
//! - `\u`, `\l`: uppercase / lowercase the next char
//! - `\U`, `\L` ... `\E` (or `\e`): uppercase / lowercase until `\E`
//! - `\&`, `\\`, `\$`, `$$`, `\<delimiter>`: literal `&`, `\`, `$`, `$` and the
//!   delimiter
//!
//! Flags: `g` replace every match on a line (not only the first), `i` ignore
//! case, `I` don't ignore case, `n` only count the matches.

use regex::{Captures, Regex, RegexBuilder};

use crate::{Rope, Tendril};

/// Which lines a substitution applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `%`: the whole document.
    Document,
    /// `N,M`, `N` or `.`: lines (0-based, inclusive). `None` is the current
    /// line.
    Lines(Option<usize>, Option<usize>),
    /// No range: the selections, or the current lines when the selections are
    /// just cursors.
    Selections,
}

/// A parsed `:s` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub scope: Scope,
    /// Empty means "the last search pattern".
    pub pattern: String,
    pub replacement: String,
    pub flags: Flags,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub global: bool,
    /// `Some(true)` for `i`, `Some(false)` for `I`.
    pub ignore_case: Option<bool>,
    pub count_only: bool,
}

/// Whether `input` (a command line without the `:`) is a substitute command
/// like `s/a/b/`, `%s#a#b#g` or `3,7s/a/b`.
pub fn is_substitute(input: &str) -> bool {
    let (_, rest) = split_range(input.trim_start());
    let mut chars = rest.chars();
    chars.next() == Some('s') && chars.next().is_some_and(is_delimiter)
}

fn is_delimiter(c: char) -> bool {
    !c.is_alphanumeric() && !c.is_whitespace() && c != '\\' && c != '"' && c != '|'
}

/// Split a leading range (`%`, `.`, `N`, `N,M`) off `input`.
fn split_range(input: &str) -> (&str, &str) {
    if let Some(rest) = input.strip_prefix('%') {
        return ("%", rest);
    }
    let end = input
        .find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.' || c == '$'))
        .unwrap_or(input.len());
    input.split_at(end)
}

/// Parse a 1-based line number, `.` (current line) or `$` (last line).
fn parse_line(s: &str) -> Result<Option<usize>, String> {
    match s {
        "." => Ok(None),
        "$" => Ok(Some(usize::MAX)),
        _ => match s.parse::<usize>() {
            Ok(0) | Err(_) => Err(format!("invalid line number '{s}'")),
            Ok(n) => Ok(Some(n - 1)),
        },
    }
}

/// Parse a substitute command line (see [`is_substitute`]).
pub fn parse(input: &str) -> Result<Command, String> {
    let (range, rest) = split_range(input.trim_start());
    let scope = match range {
        "" => Scope::Selections,
        "%" => Scope::Document,
        range => match range.split_once(',') {
            Some((from, to)) => Scope::Lines(parse_line(from)?, parse_line(to)?),
            None => {
                let line = parse_line(range)?;
                Scope::Lines(line, line)
            }
        },
    };
    let rest = rest
        .strip_prefix('s')
        .ok_or_else(|| "not a substitute command".to_string())?;
    let mut chars = rest.chars();
    let delimiter = chars
        .next()
        .filter(|&c| is_delimiter(c))
        .ok_or_else(|| "expected a delimiter after 's', like s/a/b/".to_string())?;
    let rest = chars.as_str();

    let (pattern, rest) = take_part(rest, delimiter, true);
    let (replacement, rest) = match rest {
        Some(rest) => {
            let (replacement, rest) = take_part(rest, delimiter, false);
            (replacement, rest.unwrap_or(""))
        }
        None => (String::new(), ""),
    };

    let mut flags = Flags::default();
    for flag in rest.trim().chars() {
        match flag {
            'g' => flags.global = true,
            'i' => flags.ignore_case = Some(true),
            'I' => flags.ignore_case = Some(false),
            'n' => flags.count_only = true,
            // Vim's `e` (no error when nothing matches) and `&` (keep flags)
            // don't apply.
            _ => return Err(format!("unknown flag '{flag}'")),
        }
    }
    Ok(Command {
        scope,
        pattern,
        replacement,
        flags,
    })
}

/// Take text up to an unescaped `delimiter`. Returns the part and the text
/// after the delimiter (`None` if there was no delimiter). An escaped
/// delimiter becomes the delimiter itself; in the pattern it is escaped for
/// the regex if it is special there.
fn take_part(input: &str, delimiter: char, is_pattern: bool) -> (String, Option<&str>) {
    let mut part = String::new();
    let mut chars = input.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == delimiter {
            return (part, Some(&input[i + c.len_utf8()..]));
        }
        if c == '\\' {
            match chars.next() {
                Some((_, next)) if next == delimiter => {
                    if is_pattern && regex_syntax_char(next) {
                        part.push('\\');
                    }
                    part.push(next);
                }
                Some((_, next)) => {
                    part.push('\\');
                    part.push(next);
                }
                None => part.push('\\'),
            }
            continue;
        }
        part.push(c);
    }
    (part, None)
}

fn regex_syntax_char(c: char) -> bool {
    matches!(
        c,
        '\\' | '.'
            | '+'
            | '*'
            | '?'
            | '('
            | ')'
            | '|'
            | '['
            | ']'
            | '{'
            | '}'
            | '^'
            | '$'
            | '#'
            | '&'
            | '-'
            | '~'
    )
}

/// Translate Vim's `\<` and `\>` to `\b`.
fn translate_pattern(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('<') | Some('>') => out.push_str("\\b"),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Group(usize),
    Named(String),
    UpperNext,
    LowerNext,
    Upper,
    Lower,
    EndCase,
}

/// A compiled substitution.
#[derive(Debug, Clone)]
pub struct Substitution {
    regex: Regex,
    replacement: Vec<Part>,
    global: bool,
}

/// What a substitution changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `(from, to, replacement)` in chars, sorted, for a transaction.
    pub changes: Vec<(usize, usize, Option<Tendril>)>,
    pub matches: usize,
    /// Number of lines with at least one match.
    pub lines: usize,
    /// First line (0-based) with a match, and the last one.
    pub first_line: Option<usize>,
    pub last_line: Option<usize>,
}

impl Substitution {
    /// Compile `command`. `last_search` replaces an empty pattern.
    pub fn new(command: &Command, last_search: Option<&str>) -> Result<Self, String> {
        let pattern = if command.pattern.is_empty() {
            last_search
                .filter(|pattern| !pattern.is_empty())
                .ok_or_else(|| "no previous search pattern".to_string())?
                .to_string()
        } else {
            translate_pattern(&command.pattern)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(command.flags.ignore_case.unwrap_or(false))
            .multi_line(true)
            .build()
            .map_err(|err| format!("invalid pattern: {err}"))?;
        let replacement = parse_replacement(&command.replacement)?;
        for part in &replacement {
            match part {
                Part::Group(group) if *group >= regex.captures_len() => {
                    return Err(format!("no capture group {group} in the pattern"));
                }
                Part::Named(name) if !regex.capture_names().flatten().any(|n| n == name) => {
                    return Err(format!("no capture group named '{name}' in the pattern"));
                }
                _ => {}
            }
        }
        Ok(Self {
            regex,
            replacement,
            global: command.flags.global,
        })
    }

    /// Find the matches in the char ranges `scopes` (sorted, not overlapping)
    /// of `text` and compute the replacements. Without the `g` flag only the
    /// first match on each line is replaced.
    pub fn apply(&self, text: &Rope, scopes: &[(usize, usize)]) -> Outcome {
        let mut outcome = Outcome::default();
        let mut last_line = None;
        // After a final newline there is no line in Vim's sense; `^` or `$`
        // must not match there.
        let phantom_line_start = (text.len_chars() > 0 && text.char(text.len_chars() - 1) == '\n')
            .then(|| text.len_chars());
        for &(from, to) in scopes {
            let from_byte = text.char_to_byte(from);
            let haystack = text.slice(from..to).to_string();
            for captures in self.regex.captures_iter(&haystack) {
                let whole = captures.get(0).unwrap();
                let start = text.byte_to_char(from_byte + whole.start());
                if Some(start) == phantom_line_start {
                    continue;
                }
                let end = text.byte_to_char(from_byte + whole.end());
                let line = text.char_to_line(start);
                if !self.global && last_line == Some(line) {
                    continue;
                }
                if last_line != Some(line) {
                    outcome.lines += 1;
                    outcome.first_line.get_or_insert(line);
                    outcome.last_line = Some(line);
                }
                last_line = Some(line);
                outcome.matches += 1;
                let replacement = self.expand(&captures);
                outcome.changes.push((
                    start,
                    end,
                    (!replacement.is_empty()).then(|| replacement.into()),
                ));
            }
        }
        outcome
    }

    fn expand(&self, captures: &Captures) -> String {
        #[derive(Clone, Copy, PartialEq)]
        enum Case {
            None,
            Upper,
            Lower,
        }
        let mut out = String::new();
        let mut case = Case::None;
        let mut next: Option<Case> = None;
        let push = |out: &mut String, s: &str, case: Case, next: &mut Option<Case>| {
            for c in s.chars() {
                let c_case = next.take().unwrap_or(case);
                match c_case {
                    Case::Upper => out.extend(c.to_uppercase()),
                    Case::Lower => out.extend(c.to_lowercase()),
                    Case::None => out.push(c),
                }
            }
        };
        for part in &self.replacement {
            match part {
                Part::Literal(s) => push(&mut out, s, case, &mut next),
                Part::Group(group) => {
                    if let Some(m) = captures.get(*group) {
                        push(&mut out, m.as_str(), case, &mut next);
                    }
                }
                Part::Named(name) => {
                    if let Some(m) = captures.name(name) {
                        push(&mut out, m.as_str(), case, &mut next);
                    }
                }
                Part::UpperNext => next = Some(Case::Upper),
                Part::LowerNext => next = Some(Case::Lower),
                Part::Upper => case = Case::Upper,
                Part::Lower => case = Case::Lower,
                Part::EndCase => case = Case::None,
            }
        }
        out
    }
}

fn parse_replacement(replacement: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let flush = |literal: &mut String, parts: &mut Vec<Part>| {
        if !literal.is_empty() {
            parts.push(Part::Literal(std::mem::take(literal)));
        }
    };
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(next) = chars.next() else {
                    literal.push('\\');
                    break;
                };
                let part = match next {
                    '0'..='9' => Part::Group(next as usize - '0' as usize),
                    'n' => {
                        literal.push('\n');
                        continue;
                    }
                    't' => {
                        literal.push('\t');
                        continue;
                    }
                    'r' => {
                        literal.push('\r');
                        continue;
                    }
                    'u' => Part::UpperNext,
                    'l' => Part::LowerNext,
                    'U' => Part::Upper,
                    'L' => Part::Lower,
                    'E' | 'e' => Part::EndCase,
                    other => {
                        // `\&`, `\\`, `\$`, `\/` ...
                        literal.push(other);
                        continue;
                    }
                };
                flush(&mut literal, &mut parts);
                parts.push(part);
            }
            '&' => {
                flush(&mut literal, &mut parts);
                parts.push(Part::Group(0));
            }
            '$' => match chars.peek() {
                Some('$') => {
                    chars.next();
                    literal.push('$');
                }
                Some('{') => {
                    chars.next();
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(c) => name.push(c),
                            None => return Err("unterminated ${ in the replacement".into()),
                        }
                    }
                    flush(&mut literal, &mut parts);
                    parts.push(group_part(name));
                }
                Some(c) if c.is_alphanumeric() || *c == '_' => {
                    let mut name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_alphanumeric() || c == '_' {
                            name.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    flush(&mut literal, &mut parts);
                    parts.push(group_part(name));
                }
                _ => literal.push('$'),
            },
            c => literal.push(c),
        }
    }
    flush(&mut literal, &mut parts);
    Ok(parts)
}

fn group_part(name: String) -> Part {
    match name.parse() {
        Ok(group) => Part::Group(group),
        Err(_) => Part::Named(name),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn sub(text: &str, command: &str) -> Result<(String, usize, usize), String> {
        let command = parse(command)?;
        let substitution = Substitution::new(&command, Some("last"))?;
        let rope = Rope::from(text);
        let outcome = substitution.apply(&rope, &[(0, rope.len_chars())]);
        let mut result = String::new();
        let mut pos = 0;
        let chars: Vec<char> = text.chars().collect();
        for (from, to, replacement) in &outcome.changes {
            result.extend(&chars[pos..*from]);
            if let Some(replacement) = replacement {
                result.push_str(replacement);
            }
            pos = *to;
        }
        result.extend(&chars[pos..]);
        Ok((result, outcome.matches, outcome.lines))
    }

    fn s(text: &str, command: &str) -> String {
        sub(text, command).unwrap().0
    }

    #[test]
    fn detects_substitute_commands() {
        for yes in [
            "s/a/b/",
            "%s/a/b/g",
            "s#a#b#",
            "3,7s/a/b",
            ".s/a/b",
            "s/a",
            "1,$s/a/b/",
            "s|a|b",
        ] {
            assert_eq!(is_substitute(yes), !yes.contains('|'), "{yes}");
        }
        for no in ["sh ls", "set x", "s", "sort", "split", "s a", "write", "%"] {
            assert!(!is_substitute(no), "{no}");
        }
    }

    #[test]
    fn parsing() {
        assert_eq!(
            parse("%s/a\\/b/c/gi").unwrap(),
            Command {
                scope: Scope::Document,
                pattern: "a/b".into(),
                replacement: "c".into(),
                flags: Flags {
                    global: true,
                    ignore_case: Some(true),
                    count_only: false
                },
            }
        );
        assert_eq!(
            parse("2,4s/x/y").unwrap().scope,
            Scope::Lines(Some(1), Some(3))
        );
        assert_eq!(
            parse("5s/x/y").unwrap().scope,
            Scope::Lines(Some(4), Some(4))
        );
        assert_eq!(
            parse(".,$s/x/y").unwrap().scope,
            Scope::Lines(None, Some(usize::MAX))
        );
        assert_eq!(parse("s/x/y/").unwrap().scope, Scope::Selections);
        // Missing parts.
        let command = parse("s/x").unwrap();
        assert_eq!(
            (command.pattern.as_str(), command.replacement.as_str()),
            ("x", "")
        );
        let command = parse("s/x/").unwrap();
        assert_eq!(command.replacement, "");
        assert!(parse("s/a/b/q").unwrap_err().contains("unknown flag 'q'"));
        assert!(parse("0s/a/b").unwrap_err().contains("invalid line number"));
        // An escaped delimiter that is special in regexes stays escaped.
        assert_eq!(parse("s.a\\.b.c.").unwrap().pattern, "a\\.b");
    }

    #[test]
    fn first_match_per_line_unless_global() {
        assert_eq!(s("aaa\naaa\n", "s/a/b/"), "baa\nbaa\n");
        assert_eq!(s("aaa\naaa\n", "s/a/b/g"), "bbb\nbbb\n");
        assert_eq!(sub("aaa\naaa\n", "s/a/b/g").unwrap().1, 6);
        assert_eq!(sub("aaa\nxyz\naaa\n", "s/a/b/").unwrap().2, 2);
    }

    #[test]
    fn capture_groups() {
        assert_eq!(s("John Smith", "s/(\\w+) (\\w+)/\\2, \\1/"), "Smith, John");
        assert_eq!(s("John Smith", "s/(\\w+) (\\w+)/$2 $1/"), "Smith John");
        assert_eq!(s("John Smith", "s/(\\w+) (\\w+)/${2}_${1}/"), "Smith_John");
        assert_eq!(
            s(
                "2026-10-19",
                "s/(?<y>\\d+)-(?<m>\\d+)-(?<d>\\d+)/${d}.${m}.${y}/"
            ),
            "19.10.2026"
        );
        assert_eq!(
            s("2026-10-19", "s/(?<y>\\d+)-(?<m>\\d+)-(?<d>\\d+)/$d.$m.$y/"),
            "19.10.2026"
        );
        assert_eq!(s("abc", "s/b/[&]/"), "a[b]c");
        assert_eq!(s("abc", "s/b/[\\0]/"), "a[b]c");
        assert_eq!(s("abc", "s/b/\\&/"), "a&c");
        assert_eq!(s("abc", "s/b/$$/"), "a$c");
        // Optional groups that didn't match are empty.
        assert_eq!(s("ac", "s/a(b)?c/[\\1]/"), "[]");
    }

    #[test]
    fn invalid_groups_are_reported() {
        assert!(sub("abc", "s/(a)/\\2/")
            .unwrap_err()
            .contains("no capture group 2"));
        assert!(sub("abc", "s/(a)/${x}/").unwrap_err().contains("named 'x'"));
        assert!(sub("abc", "s/(/x/")
            .unwrap_err()
            .contains("invalid pattern"));
        assert!(sub("abc", "s/a/${1/").unwrap_err().contains("unterminated"));
    }

    #[test]
    fn case_changes() {
        assert_eq!(
            s("hello world", "s/(\\w+) (\\w+)/\\u\\1 \\U\\2\\E!/"),
            "Hello WORLD!"
        );
        assert_eq!(
            s("HELLO World", "s/(\\w+) (\\w+)/\\L\\1\\E \\l\\2/"),
            "hello world"
        );
        assert_eq!(s("foo_bar", "s/_(\\w)/\\u\\1/g"), "fooBar");
        assert_eq!(s("snake case words", "s/\\w+/\\u&/g"), "Snake Case Words");
    }

    #[test]
    fn special_characters() {
        assert_eq!(s("a,b,c", "s/,/\\n/g"), "a\nb\nc");
        assert_eq!(s("a b", "s/ /\\t/"), "a\tb");
        assert_eq!(s("a/b", "s/\\//-/"), "a-b");
        assert_eq!(s("a/b", "s#/#\\\\#"), "a\\b");
        // Deleting.
        assert_eq!(s("a1b2", "s/\\d//g"), "ab");
        // Unicode.
        assert_eq!(s("café latte", "s/é/e/"), "cafe latte");
        assert_eq!(s("über", "s/\\w+/\\U&/"), "ÜBER");
    }

    #[test]
    fn vim_word_boundaries_and_flags() {
        assert_eq!(s("cat concat cat", "s/\\<cat\\>/dog/g"), "dog concat dog");
        assert_eq!(s("Foo foo", "s/foo/x/gi"), "x x");
        assert_eq!(s("Foo foo", "s/foo/x/gI"), "Foo x");
        // Empty pattern: the last search.
        assert_eq!(s("the last one", "s//first/"), "the first one");
    }

    #[test]
    fn anchors_work_per_line() {
        assert_eq!(s("ab\nab\n", "s/^/> /"), "> ab\n> ab\n");
        assert_eq!(s("ab\nab", "s/$/;/"), "ab;\nab;");
        assert_eq!(s("ab\nab\n", "s/$/;/"), "ab;\nab;\n");
        assert_eq!(s("x\n\ny\n", "s/^$/-/"), "x\n-\ny\n");
    }

    #[test]
    fn scopes_limit_matches() {
        let command = parse("s/a/b/g").unwrap();
        let substitution = Substitution::new(&command, None).unwrap();
        let rope = Rope::from("aaaa");
        let outcome = substitution.apply(&rope, &[(1, 3)]);
        assert_eq!(outcome.matches, 2);
        assert_eq!(outcome.changes[0].0, 1);
        assert_eq!(outcome.changes[1].1, 3);
    }

    #[test]
    fn no_matches() {
        let (text, matches, lines) = sub("abc", "s/x/y/").unwrap();
        assert_eq!((text.as_str(), matches, lines), ("abc", 0, 0));
    }
}
