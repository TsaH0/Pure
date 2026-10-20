//! Vim-style `:s/pattern/replacement/flags`.
//!
//! Patterns use Helix's regex syntax (the `regex` crate) or Vim's:
//!
//! - `\v`, `\m`, `\M`, `\V` at the start pick Vim's very magic, magic
//!   (default), nomagic and very nomagic modes. Without one, a pattern that
//!   uses Vim-only escapes (`\(`, `\+`, `\|`, `\=`, `\{`, `\zs`, Vim classes
//!   like `\u`) and none of the regex crate's plain operators is read as
//!   Vim's magic syntax, so the "one-eyed Kirby" `\(.*\)` works.
//! - Vim's counts `\{n,m}` (`\{-}` is non-greedy, the "sleeping Kirby"
//!   `\(.\{-}\)`), word boundaries `\<` `\>` (`<` `>` in very magic),
//!   classes (`\a` `\l` `\u` `\h` `\x` `\o` `\k` ...; `\s` is only space and
//!   tab) and `\zs` / `\ze`, which set where the replaced part starts and ends
//!   (the rest only has to match around it).
//!
//! The replacement understands:
//!
//! - `\1` .. `\9`, `$1`, `${1}`, `${name}`, `$name`: capture groups
//! - `&`, `\0`, `$0`: the whole match
//! - `\n`, `\r`: a line break; `\t`: a tab
//! - `\u`, `\l`: uppercase / lowercase the next char
//! - `\U`, `\L` ... `\E` (or `\e`): uppercase / lowercase until `\E`
//! - `\&`, `\\`, `\$`, `$$`, `\<delimiter>`: literal `&`, `\`, `$`, `$` and the
//!   delimiter
//!
//! Flags: `g` replace every match on a line (not only the first), `i` ignore
//! case, `I` don't ignore case, `n` only count the matches, `e` no error when
//! nothing matches.

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
    /// Whether the delimiter after the pattern was typed, so a (possibly
    /// empty) replacement was given. Used by the live preview to tell "still
    /// typing the pattern" from "replace with nothing".
    pub has_replacement: bool,
    pub flags: Flags,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub global: bool,
    /// `Some(true)` for `i`, `Some(false)` for `I`.
    pub ignore_case: Option<bool>,
    pub count_only: bool,
    /// `e`: no error when nothing matches.
    pub quiet: bool,
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

/// Split a leading range (`%`, `.`, `N`, `N,M`, Vim's visual `'<,'>`) off
/// `input`.
fn split_range(input: &str) -> (&str, &str) {
    if let Some(rest) = input.strip_prefix('%') {
        return ("%", rest);
    }
    if let Some(rest) = input.strip_prefix("'<,'>") {
        return ("'<,'>", rest);
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
        // There are no marks: Vim's visual range means the selections.
        "" | "'<,'>" => Scope::Selections,
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
    let has_replacement = rest.is_some();
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
            'e' => flags.quiet = true,
            // Vim's `&` (keep flags) doesn't apply.
            _ => return Err(format!("unknown flag '{flag}'")),
        }
    }
    Ok(Command {
        scope,
        pattern,
        replacement,
        has_replacement,
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

/// How a pattern is read, see [`translate_pattern`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syntax {
    /// The regex crate's syntax (as in Helix's search).
    Regex,
    /// Vim's `\v`.
    VeryMagic,
    /// Vim's default, `\m`.
    Magic,
    /// Vim's `\M`.
    NoMagic,
    /// Vim's `\V`.
    VeryNoMagic,
}

/// Name of the group standing for the part between `\zs` and `\ze`.
const MATCH_GROUP: &str = "__helix_match";
/// Placeholders for `\zs` and `\ze` during translation.
const ZS: char = '\u{E000}';
const ZE: char = '\u{E001}';

/// Whether `pattern` (without a `\v`/`\m`/`\M`/`\V` prefix) is written in
/// Vim's default syntax: it uses an escape that only means something in Vim
/// (`\(`, `\|`, `\+`, `\?`, `\=`, `\{`, `\zs`, `\ze`) and none of the regex
/// crate's plain operators `(` `)` `|` `+` `?` `{`.
fn looks_like_vim(pattern: &str) -> bool {
    let mut vim = false;
    let mut regex = false;
    let mut in_class = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('(' | ')' | '|' | '+' | '?' | '=' | '{') if !in_class => vim = true,
                Some('z') if !in_class && matches!(chars.peek(), Some('s' | 'e')) => vim = true,
                // Vim's classes that aren't escapes in the regex crate (`\u`
                // and `\x` are, followed by hex digits or `{`).
                Some('l' | 'L' | 'h' | 'H' | 'o' | 'O' | 'k' | 'K' | 'i' | 'I' | 'a') => vim = true,
                Some('u' | 'U' | 'x' | 'X')
                    if !chars
                        .peek()
                        .is_some_and(|c| c.is_ascii_hexdigit() || *c == '{') =>
                {
                    vim = true
                }
                _ => {}
            },
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' | ')' | '|' | '+' | '?' | '{' if !in_class => regex = true,
            _ => {}
        }
    }
    vim && !regex
}

/// Escape `c` for the regex crate if needed.
fn push_literal(out: &mut String, c: char) {
    if regex_syntax_char(c) {
        out.push('\\');
    }
    out.push(c);
}

/// Translate a Vim count (`\{n,m}`, `{-}`, ...) whose opening brace was just
/// read. Returns the regex crate's quantifier.
fn translate_count(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    let mut body = String::new();
    loop {
        match chars.next() {
            Some('\\') if chars.peek() == Some(&'}') => {
                chars.next();
                break;
            }
            Some('}') => break,
            Some(c) => body.push(c),
            None => return Err("unterminated \\{ in the pattern".into()),
        }
    }
    let (lazy, range) = match body.strip_prefix('-') {
        Some(range) => (true, range),
        None => (false, body.as_str()),
    };
    let quantifier = match range.split_once(',') {
        _ if range.is_empty() => "*".to_string(),
        None => format!("{{{range}}}"),
        Some(("", "")) => "*".to_string(),
        Some(("", max)) => format!("{{0,{max}}}"),
        Some((min, "")) => format!("{{{min},}}"),
        Some((min, max)) => format!("{{{min},{max}}}"),
    };
    if !range.chars().all(|c| c.is_ascii_digit() || c == ',') {
        return Err(format!("invalid count {{{body}}} in the pattern"));
    }
    Ok(if lazy {
        format!("{quantifier}?")
    } else {
        quantifier
    })
}

/// Vim's character class escapes, like `\a` (alphabetic) or `\h` (head of a
/// word). `\d`, `\s`, `\w` and their negations are the same in both syntaxes.
fn vim_class(c: char) -> Option<&'static str> {
    Some(match c {
        'a' => "[A-Za-z]",
        'A' => "[^A-Za-z]",
        'l' => "[a-z]",
        'L' => "[^a-z]",
        'u' => "[A-Z]",
        'U' => "[^A-Z]",
        'h' => "[A-Za-z_]",
        'H' => "[^A-Za-z_]",
        'x' => "[0-9A-Fa-f]",
        'X' => "[^0-9A-Fa-f]",
        'o' => "[0-7]",
        'O' => "[^0-7]",
        // Identifier and keyword chars (without 'iskeyword' settings).
        // Vim's `\s` is only space and tab, not newlines.
        's' => "[ \\t]",
        'S' => "[^ \\t\\n]",
        'i' | 'k' => "\\w",
        'I' | 'K' => "[^\\W\\d]",
        'f' | 'p' => "\\S",
        'F' | 'P' => "[^\\s\\d]",
        _ => return None,
    })
}

/// Translate a pattern to the regex crate's syntax.
///
/// - `\v`, `\m`, `\M`, `\V` at the start pick Vim's very magic, magic,
///   nomagic and very nomagic syntax. Without one, a pattern that looks like
///   Vim's default syntax (see [`looks_like_vim`]) is read as magic; anything
///   else is the regex crate's syntax.
/// - In Vim's syntaxes `\{n,m}` / `{n,m}` counts (`-` for non-greedy, so
///   `.\{-}` is `.*?`), `<` / `>` (very magic) or `\<` / `\>` word boundaries,
///   `=` (very magic) or `\=` for "optional", and classes like `\a`, `\l`,
///   `\u`, `\h`, `\x`, `\k` work as in Vim.
/// - `\zs` and `\ze` (all syntaxes) set the start and end of the part that is
///   replaced; the rest of the pattern only has to match around it.
fn translate_pattern(pattern: &str) -> Result<String, String> {
    let (pattern, syntax) = if let Some(rest) = pattern.strip_prefix("\\v") {
        (rest, Syntax::VeryMagic)
    } else if let Some(rest) = pattern.strip_prefix("\\m") {
        (rest, Syntax::Magic)
    } else if let Some(rest) = pattern.strip_prefix("\\M") {
        (rest, Syntax::NoMagic)
    } else if let Some(rest) = pattern.strip_prefix("\\V") {
        (rest, Syntax::VeryNoMagic)
    } else if looks_like_vim(pattern) {
        (pattern, Syntax::Magic)
    } else {
        (pattern, Syntax::Regex)
    };
    let vim = syntax != Syntax::Regex;
    // Which plain chars are operators.
    let plain_operator = |c: char| match syntax {
        Syntax::Regex | Syntax::VeryMagic => regex_syntax_char(c) || c == '#',
        Syntax::Magic => matches!(c, '.' | '*' | '[' | '^' | '$'),
        Syntax::NoMagic | Syntax::VeryNoMagic => matches!(c, '^' | '$'),
    };

    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        // A character class, copied as is.
        let class_start = match c {
            '[' => plain_operator('['),
            '\\' if chars.peek() == Some(&'[') && !plain_operator('[') => {
                chars.next();
                true
            }
            _ => false,
        };
        if class_start {
            out.push('[');
            if chars.peek() == Some(&'^') {
                out.push(chars.next().unwrap());
            }
            if chars.peek() == Some(&']') {
                chars.next();
                out.push_str("\\]");
            }
            loop {
                match chars.next() {
                    Some(']') => break,
                    Some('\\') => {
                        out.push('\\');
                        out.push(chars.next().unwrap_or('\\'));
                    }
                    // POSIX classes like `[:alpha:]`.
                    Some('[') if chars.peek() == Some(&':') => {
                        out.push('[');
                        for c in chars.by_ref() {
                            out.push(c);
                            if c == ']' {
                                break;
                            }
                        }
                    }
                    Some('[') => out.push_str("\\["),
                    Some(c) => out.push(c),
                    None => return Err("unterminated [ in the pattern".into()),
                }
            }
            out.push(']');
            continue;
        }

        if c == '\\' {
            let Some(next) = chars.next() else {
                out.push_str("\\\\");
                break;
            };
            match next {
                'z' if chars.peek() == Some(&'s') => {
                    chars.next();
                    out.push(ZS);
                }
                'z' if chars.peek() == Some(&'e') => {
                    chars.next();
                    out.push(ZE);
                }
                '<' | '>' if syntax != Syntax::VeryMagic => out.push_str("\\b"),
                'n' => out.push_str("\\n"),
                't' => out.push_str("\\t"),
                '{' if !plain_operator('{') && vim => out.push_str(&translate_count(&mut chars)?),
                '=' if vim && syntax != Syntax::VeryMagic => out.push('?'),
                c if vim && vim_class(c).is_some() => out.push_str(vim_class(c).unwrap()),
                // An escaped char that is literal when plain is an operator.
                c if vim
                    && !c.is_alphanumeric()
                    && !plain_operator(c)
                    && (regex_syntax_char(c) || c == '~') =>
                {
                    out.push(c)
                }
                // Regex escapes (`\d`, `\w`, `\b` ...).
                c if c.is_alphanumeric() => {
                    out.push('\\');
                    out.push(c);
                }
                // Anything else escaped is that char.
                c => push_literal(&mut out, c),
            }
            continue;
        }

        match c {
            '{' if syntax == Syntax::VeryMagic => out.push_str(&translate_count(&mut chars)?),
            '=' if syntax == Syntax::VeryMagic => out.push('?'),
            '<' | '>' if syntax == Syntax::VeryMagic => out.push_str("\\b"),
            c if plain_operator(c) => out.push(c),
            c if vim => push_literal(&mut out, c),
            c => out.push(c),
        }
    }

    // `\zs` / `\ze`: the part in between becomes a named group.
    if out.contains(ZS) || out.contains(ZE) {
        let (before, rest) = match out.split_once(ZS) {
            Some((before, rest)) => (before.to_string(), rest.to_string()),
            None => (String::new(), out.clone()),
        };
        let (inner, after) = match rest.split_once(ZE) {
            Some((inner, after)) => (inner.to_string(), after.to_string()),
            None => (rest.clone(), String::new()),
        };
        if before.contains(ZE) || inner.contains(ZS) || after.contains(ZS) || after.contains(ZE) {
            return Err("\\zs and \\ze can each be used only once, \\zs first".into());
        }
        out = format!("{before}(?P<{MATCH_GROUP}>{inner}){after}");
    }
    Ok(out)
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
    /// Index of the `\zs`/`\ze` group, which is the part to replace.
    match_group: Option<usize>,
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
    /// How many of `changes` have their replacement computed: all, unless
    /// [`Substitution::apply_limited`] ran out of budget.
    pub expanded: usize,
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
            translate_pattern(&command.pattern)?
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(command.flags.ignore_case.unwrap_or(false))
            .multi_line(true)
            .build()
            .map_err(|err| format!("invalid pattern: {err}"))?;
        let replacement = parse_replacement(&command.replacement)?;
        let match_group = regex
            .capture_names()
            .position(|name| name == Some(MATCH_GROUP));
        let user_groups = regex.captures_len() - usize::from(match_group.is_some());
        for part in &replacement {
            match part {
                Part::Group(group) if *group >= user_groups => {
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
            match_group,
        })
    }

    /// Find the matches in the char ranges `scopes` (sorted, not overlapping)
    /// of `text` and compute the replacements. Without the `g` flag only the
    /// first match on each line is replaced.
    pub fn apply(&self, text: &Rope, scopes: &[(usize, usize)]) -> Outcome {
        self.apply_limited(text, scopes, usize::MAX)
    }

    /// Like [`Substitution::apply`], but once the replacements reach
    /// `budget` bytes the rest are only counted and located (their
    /// replacement is `None` and they are past `Outcome::expanded`), for a
    /// preview that shows the first few.
    pub fn apply_limited(&self, text: &Rope, scopes: &[(usize, usize)], budget: usize) -> Outcome {
        let mut outcome = Outcome::default();
        let mut spent = 0usize;
        let mut last_line = None;
        // After a final newline there is no line in Vim's sense; `^` or `$`
        // must not match there.
        let phantom_line_start = (text.len_chars() > 0 && text.char(text.len_chars() - 1) == '\n')
            .then(|| text.len_chars());
        for &(from, to) in scopes {
            let from_byte = text.char_to_byte(from);
            let haystack = text.slice(from..to).to_string();
            for captures in self.regex.captures_iter(&haystack) {
                let Some(whole) = self.group(&captures, 0) else {
                    continue;
                };
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
                if spent >= budget {
                    outcome.changes.push((start, end, None));
                    continue;
                }
                let replacement = self.expand(&captures);
                spent = spent.saturating_add(replacement.len().max(1));
                outcome.expanded += 1;
                outcome.changes.push((
                    start,
                    end,
                    (!replacement.is_empty()).then(|| replacement.into()),
                ));
            }
        }
        outcome
    }

    /// Group `group` as numbered in the pattern; 0 is what gets replaced (the
    /// whole match or the part between `\zs` and `\ze`).
    fn group<'h>(&self, captures: &Captures<'h>, group: usize) -> Option<regex::Match<'h>> {
        match self.match_group {
            Some(match_group) if group == 0 => captures.get(match_group),
            Some(match_group) if group >= match_group => captures.get(group + 1),
            _ => captures.get(group),
        }
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
                    if let Some(m) = self.group(captures, *group) {
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
                    // Like Vim, `\r` breaks the line.
                    'r' => {
                        literal.push('\n');
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
    fn limited_application_stops_expanding_at_its_budget() {
        let text = Rope::from("x".repeat(100_000));
        let replacement = "y".repeat(256);
        let command = parse(&format!("s/x/{replacement}/g")).unwrap();
        let substitution = Substitution::new(&command, None).unwrap();
        let full = substitution.apply(&text, &[(0, text.len_chars())]);
        assert_eq!(full.expanded, 100_000);
        let limited = substitution.apply_limited(&text, &[(0, text.len_chars())], 1 << 20);
        // Every match is still found and counted...
        assert_eq!(limited.matches, 100_000);
        assert_eq!(limited.changes.len(), 100_000);
        // ...but only about 1 MiB of replacements is built.
        assert_eq!(limited.expanded, (1 << 20) / 256);
        let built: usize = limited
            .changes
            .iter()
            .filter_map(|(_, _, r)| r.as_ref().map(|r| r.len()))
            .sum();
        assert_eq!(built, 1 << 20);
        assert_eq!(
            &limited.changes[..limited.expanded],
            &full.changes[..limited.expanded]
        );
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
                has_replacement: true,
                flags: Flags {
                    global: true,
                    ignore_case: Some(true),
                    count_only: false,
                    quiet: false,
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
        assert_eq!(parse("'<,'>s/x/y/").unwrap().scope, Scope::Selections);
        assert!(is_substitute("'<,'>s/x/y/"));
        assert!(!parse("s/x").unwrap().has_replacement);
        assert!(parse("s/x/").unwrap().has_replacement);
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
    fn one_eyed_kirby() {
        // The capture-everything group, in both syntaxes.
        assert_eq!(s("hello world", "s/(.*)/[\\1]/"), "[hello world]");
        assert_eq!(s("hello world", "s/\\(.*\\)/[\\1]/"), "[hello world]");
        assert_eq!(s("hello world", "s/\\v(.*)/<\\1>/"), "<hello world>");
        // Per line, keeping the rest of the document.
        assert_eq!(s("a\nb\n", "%s/^\\(.*\\)$/- \\1/"), "- a\n- b\n");
        assert_eq!(
            s("key = value", "s/\\(.*\\) = \\(.*\\)/\\2 = \\1/"),
            "value = key"
        );
    }

    /// The Kirby family, very magic, `\zs`/`\ze`, classes and the recipes from
    /// a typical Vim regex guide.
    #[test]
    fn vim_regex_guide() {
        // One-eyed Kirby: capture everything.
        assert_eq!(s("hello world", "s/\\(.*\\)/[\\1]/"), "[hello world]");
        // Sleeping Kirby: non-greedy `\{-}`.
        assert_eq!(s("<a><b>", "s/<\\(.\\{-}\\)>/[\\1]/"), "[a]<b>");
        assert_eq!(s("<a><b>", "s/<\\(.*\\)>/[\\1]/"), "[a><b]");
        assert_eq!(s("aaaa", "s/a\\{-1,}/X/"), "Xaaa");
        // Full Kirby: two chunks around an underscore.
        assert_eq!(s("snake_case", "s/\\(.*_.*\\)/<\\1>/"), "<snake_case>");
        assert_eq!(
            s("snake_case", "s/\\(.*\\)_\\(.*\\)/\\2_\\1/"),
            "case_snake"
        );
        // Crying Kirby: one or more word chars.
        assert_eq!(s("foo bar", "s/\\(\\w\\+\\)/<\\1>/g"), "<foo> <bar>");

        // Very magic equivalents.
        assert_eq!(s("hello world", "s/\\v(.*)/[\\1]/"), "[hello world]");
        assert_eq!(s("foo bar", "s/\\v\\w+/X/g"), "X X");
        assert_eq!(s("color colour", "s/\\vcolou?r/C/g"), "C C");
        assert_eq!(s("color colour", "s/colou\\?r/C/g"), "C C");
        assert_eq!(s("foo baz bar", "s/\\v(foo|bar)/X/g"), "X baz X");
        assert_eq!(s("foo baz bar", "s/foo\\|bar/X/g"), "X baz X");
        assert_eq!(s("12 1234 123456", "s/\\v<\\d{3,5}>/N/g"), "12 N 123456");
        assert_eq!(s("12 1234 123456", "s/\\<\\d\\{3,5}\\>/N/g"), "12 N 123456");
        assert_eq!(s("a1b22", "s/\\v\\d{2}/X/"), "a1bX");
        assert_eq!(s("<a><b>", "s/\\v\\<(.{-})\\>/[\\1]/"), "[a]<b>");

        // `\zs` and `\ze`: context that isn't replaced.
        assert_eq!(s("price: 100", "s/price: \\zs\\d\\+/200/g"), "price: 200");
        assert_eq!(
            s("price: 100", "s/\\(price: \\)\\d\\+/\\1200/g"),
            "price: 200"
        );
        assert_eq!(
            s("say \"hello\" and \"bye\"", "s/\"\\zs[^\"]*\\ze\"/world/g"),
            "say \"world\" and \"world\""
        );
        assert_eq!(s("foobar foobaz", "s/foo\\zebar/X/g"), "Xbar foobaz");
        // Groups keep their numbers around `\zs`.
        assert_eq!(
            s("key=val", "s/\\(\\w\\+\\)=\\zs\\(\\w\\+\\)/\\2-\\1/"),
            "key=val-key"
        );

        // Word boundaries: Vim's `\<cat\>`, very magic `<cat>`.
        assert_eq!(
            s("cat category tomcat cat", "s/\\<cat\\>/dog/g"),
            "dog category tomcat dog"
        );
        assert_eq!(
            s("cat category tomcat cat", "s/\\v<cat>/dog/g"),
            "dog category tomcat dog"
        );

        // Vim's classes.
        assert_eq!(s("ab12_C", "s/\\a\\+/X/"), "X12_C");
        assert_eq!(s("ab12_C", "s/\\v\\l+/X/"), "X12_C");
        assert_eq!(s("abC1", "s/\\u/X/"), "abX1");
        assert_eq!(s("x = 0xFF", "s/\\v0x\\x+/HEX/"), "x = HEX");
        assert_eq!(s("my-var", "s/\\k\\+/X/g"), "X-X");
        assert_eq!(s("a [b] c", "s/[[:alpha:]]/X/g"), "X [X] X");

        // Case changes and whole-match references in replacements.
        assert_eq!(s("hello", "s/\\v(\\w+)/\\u\\1/"), "Hello");
        assert_eq!(s("hello", "s/\\v(\\w+)/\\U\\1\\e!/"), "HELLO!");
        assert_eq!(s("a 12 b 3", "s/\\v\\d+/[&]/g"), "a [12] b [3]");

        // Nomagic and very nomagic: plain chars are literal.
        assert_eq!(s("a.b axb", "s/\\Va.b/X/g"), "X axb");
        assert_eq!(s("a.b axb", "s/\\Ma\\.b/X/g"), "X X");
        assert_eq!(s("1+1=2", "s/\\V1+1/two/"), "two=2");
    }

    #[test]
    fn vim_regex_recipes() {
        // Swap two words around an equals sign.
        assert_eq!(
            s(
                "a = b\nleft=right\n",
                "%s/\\v(\\w+)\\s*\\=\\s*(\\w+)/\\2 = \\1/g"
            ),
            "b = a\nright = left\n"
        );
        // snake_case to camelCase.
        assert_eq!(s("my_var_name\n", "%s/\\v_(\\w)/\\u\\1/g"), "myVarName\n");
        // Trailing whitespace, with `e` so no match isn't an error.
        assert_eq!(s("a  \nb\t\nc\n", "%s/\\s\\+$//e"), "a\nb\nc\n");
        let (text, matches, _) = sub("clean\n", "%s/\\s\\+$//e").unwrap();
        assert_eq!((text.as_str(), matches), ("clean\n", 0));
        // Duplicate each line around a colon.
        assert_eq!(s("ab\ncd\n", "%s/.*/& : &/"), "ab : ab\ncd : cd\n");
        // `\r` in the replacement breaks the line, like Vim.
        assert_eq!(s("a,b", "s/,/\\r/"), "a\nb");
        // An empty pattern reuses the last search.
        assert_eq!(s("the last word", "s//first/"), "the first word");
    }

    #[test]
    fn invalid_vim_patterns() {
        assert!(sub("x", "s/\\v(x/y/")
            .unwrap_err()
            .contains("invalid pattern"));
        assert!(sub("x", "s/a\\{1/y/").unwrap_err().contains("unterminated"));
        assert!(sub("x", "s/a\\{x}/y/")
            .unwrap_err()
            .contains("invalid count"));
        assert!(sub("x", "s/a\\zeb\\zsc/y/")
            .unwrap_err()
            .contains("\\zs first"));
        assert!(sub("x", "s/\\v[ab/y/")
            .unwrap_err()
            .contains("unterminated ["));
    }

    #[test]
    fn vim_magic_syntax() {
        // Groups, alternation, repetition the Vim way.
        assert_eq!(
            s("John Smith", "s/\\(\\w\\+\\) \\(\\w\\+\\)/\\2 \\1/"),
            "Smith John"
        );
        assert_eq!(s("cat dog", "s/\\(cat\\|dog\\)/pet/g"), "pet pet");
        assert_eq!(s("color colour", "s/\\(colou\\=r\\)/X/g"), "X X");
        assert_eq!(s("aaa b", "s/\\(a\\{2}\\)/X/"), "Xa b");
        // In Vim's syntax plain parens, `+` and `|` are literal.
        // A pattern mixing `\(` with a plain `(` needs `\m` to be read the Vim way.
        assert_eq!(s("f(x) a+b", "s/\\m\\(f\\)(x)/\\1[x]/"), "f[x] a+b");
        assert_eq!(s("a+b", "s/\\ma+b/X/"), "X");
        // Character classes are untouched.
        assert_eq!(s("a(b)", "s/\\(a\\)[()]/\\1-/"), "a-b)");
        // `\(x\)` alone is Vim's syntax: a group.
        assert_eq!(s("(x)", "s/\\(x\\)/y/"), "(y)");
        // With a plain `(` it is the regex crate's syntax: `\(` is a literal paren.
        assert_eq!(s("(x)", "s/\\((x)\\)/[$1]/"), "[x]");
    }

    #[test]
    fn no_matches() {
        let (text, matches, lines) = sub("abc", "s/x/y/").unwrap();
        assert_eq!((text.as_str(), matches, lines), ("abc", 0, 0));
    }
}
