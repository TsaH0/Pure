use std::fmt::Display;

use crate::{
    graphemes::next_grapheme_boundary,
    match_brackets::{
        self, find_matching_bracket, find_matching_bracket_fuzzy, get_pair, is_close_bracket,
        is_open_bracket,
    },
    movement::Direction,
    search, Range, Selection, Syntax,
};
use ropey::RopeSlice;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    PairNotFound,
    CursorOverlap,
    RangeExceedsText,
    CursorOnAmbiguousPair,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match *self {
            Error::PairNotFound => "Surround pair not found around all cursors",
            Error::CursorOverlap => "Cursors overlap for a single surround pair range",
            Error::RangeExceedsText => "Cursor range exceeds text length",
            Error::CursorOnAmbiguousPair => "Cursor on ambiguous surround pair",
        })
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Letters that can be typed instead of a pair character, like Vim's `ib` /
/// `iB` (`mib` is the same as `mi(`).
pub const PAIR_ALIASES: [(char, char); 6] = [
    ('b', '('),
    ('B', '{'),
    ('r', '['),
    ('A', '<'),
    ('d', '"'),
    ('s', '\''),
];

/// Quote characters considered by the "any quote" text object (`miq`).
pub const QUOTES: [char; 3] = ['"', '\'', '`'];

/// Map a pair alias letter (see [`PAIR_ALIASES`]) to its pair character.
/// Other characters are returned unchanged.
pub fn resolve_pair_alias(ch: char) -> char {
    PAIR_ALIASES
        .iter()
        .find(|(alias, _)| *alias == ch)
        .map_or(ch, |&(_, pair)| pair)
}

/// Finds the position of surround pairs of any [`crate::match_brackets::PAIRS`]
/// using tree-sitter when possible.
///
/// # Returns
///
/// Tuple `(anchor, head)`, meaning it is not always ordered.
pub fn find_nth_closest_pairs_pos(
    syntax: Option<&Syntax>,
    text: RopeSlice,
    range: Range,
    skip: usize,
) -> Result<(usize, usize)> {
    match syntax {
        Some(syntax) => find_nth_closest_pairs_ts(syntax, text, range, skip),
        None => find_nth_closest_pairs_plain(text, range, skip),
    }
}

fn find_nth_closest_pairs_ts(
    syntax: &Syntax,
    text: RopeSlice,
    range: Range,
    mut skip: usize,
) -> Result<(usize, usize)> {
    let mut opening = range.from();
    // We want to expand the selection if we are already on the found pair,
    // otherwise we would need to subtract "-1" from "range.to()".
    let mut closing = range.to();

    while skip > 0 {
        closing = find_matching_bracket_fuzzy(syntax, text, closing).ok_or(Error::PairNotFound)?;
        opening = find_matching_bracket(syntax, text, closing).ok_or(Error::PairNotFound)?;
        // If we're already on a closing bracket "find_matching_bracket_fuzzy" will return
        // the position of the opening bracket.
        if closing < opening {
            (opening, closing) = (closing, opening);
        }

        // In case found brackets are partially inside current selection.
        if range.from() < opening || closing < range.to() - 1 {
            closing = next_grapheme_boundary(text, closing);
        } else {
            skip -= 1;
            if skip != 0 {
                closing = next_grapheme_boundary(text, closing);
            }
        }
    }

    // Keep the original direction.
    if let Direction::Forward = range.direction() {
        Ok((opening, closing))
    } else {
        Ok((closing, opening))
    }
}

fn find_nth_closest_pairs_plain(
    text: RopeSlice,
    range: Range,
    mut skip: usize,
) -> Result<(usize, usize)> {
    let mut stack = Vec::with_capacity(2);
    let pos = range.from();
    let mut close_pos = pos.saturating_sub(1);

    for ch in text.chars_at(pos) {
        close_pos += 1;

        if is_open_bracket(ch) {
            // Track open pairs encountered so that we can step over
            // the corresponding close pairs that will come up further
            // down the loop. We want to find a lone close pair whose
            // open pair is before the cursor position.
            stack.push(ch);
            continue;
        }

        if !is_close_bracket(ch) {
            // We don't care if this character isn't a brace pair item,
            // so short circuit here.
            continue;
        }

        let (open, close) = get_pair(ch);

        if stack.last() == Some(&open) {
            // If we are encountering the closing pair for an opener
            // we just found while traversing, then its inside the
            // selection and should be skipped over.
            stack.pop();
            continue;
        }

        match find_nth_open_pair(text, open, close, close_pos, 1) {
            // Before we accept this pair, we want to ensure that the
            // pair encloses the range rather than just the cursor.
            Some(open_pos)
                if open_pos <= pos.saturating_add(1)
                    && close_pos >= range.to().saturating_sub(1) =>
            {
                // Since we have special conditions for when to
                // accept, we can't just pass the skip parameter on
                // through to the find_nth_*_pair methods, so we
                // track skips manually here.
                if skip > 1 {
                    skip -= 1;
                    continue;
                }

                return match range.direction() {
                    Direction::Forward => Ok((open_pos, close_pos)),
                    Direction::Backward => Ok((close_pos, open_pos)),
                };
            }
            _ => continue,
        }
    }

    Err(Error::PairNotFound)
}

/// Find the position of surround pairs of `ch` which can be either a closing
/// or opening pair. `n` will skip n - 1 pairs (eg. n=2 will discard (only)
/// the first pair found and keep looking)
pub fn find_nth_pairs_pos(
    syntax: Option<&Syntax>,
    text: RopeSlice,
    ch: char,
    range: Range,
    n: usize,
) -> Result<(usize, usize)> {
    if text.len_chars() < 2 {
        return Err(Error::PairNotFound);
    }
    if range.to() >= text.len_chars() {
        return Err(Error::RangeExceedsText);
    }

    let (open, close) = get_pair(ch);
    let pos = range.cursor(text);

    let (open, close) = if open == close {
        if Some(open) == text.get_char(pos) {
            // Cursor is directly on match character for which the opening and closing pairs are the same. For instance: ", ', `
            //
            // This is potentially ambiguous, because there's no way to know which side of the char we should be searching on.
            syntax
                .map_or_else(
                    || match_brackets::find_matching_bracket_plaintext(text.slice(..), pos),
                    |syntax| {
                        match_brackets::find_matching_bracket_fuzzy(syntax, text.slice(..), pos)
                    },
                )
                .map(|matching_pair_pos| {
                    if matching_pair_pos > pos {
                        (Some(pos), Some(matching_pair_pos))
                    } else {
                        (Some(matching_pair_pos), Some(pos))
                    }
                })
                .ok_or(Error::CursorOnAmbiguousPair)?
        } else {
            (
                search::find_nth_char(n, text, open, pos, Direction::Backward),
                search::find_nth_char(n, text, close, pos, Direction::Forward),
            )
        }
    } else {
        (
            find_nth_open_pair(text, open, close, pos, n),
            find_nth_close_pair(text, open, close, pos, n),
        )
    };

    // preserve original direction
    match range.direction() {
        Direction::Forward => Option::zip(open, close).ok_or(Error::PairNotFound),
        Direction::Backward => Option::zip(close, open).ok_or(Error::PairNotFound),
    }
}

fn find_nth_open_pair(
    text: RopeSlice,
    open: char,
    close: char,
    mut pos: usize,
    n: usize,
) -> Option<usize> {
    if pos >= text.len_chars() {
        return None;
    }

    let mut chars = text.chars_at(pos + 1);

    // Adjusts pos for the first iteration, and handles the case of the
    // cursor being *on* the close character which will get falsely stepped over
    // if not skipped here
    if chars.prev()? == open {
        return Some(pos);
    }

    for _ in 0..n {
        let mut step_over: usize = 0;

        loop {
            let c = chars.prev()?;
            pos = pos.saturating_sub(1);

            // ignore other surround pairs that are enclosed *within* our search scope
            if c == close {
                step_over += 1;
            } else if c == open {
                if step_over == 0 {
                    break;
                }

                step_over = step_over.saturating_sub(1);
            }
        }
    }

    Some(pos)
}

fn find_nth_close_pair(
    text: RopeSlice,
    open: char,
    close: char,
    mut pos: usize,
    n: usize,
) -> Option<usize> {
    if pos >= text.len_chars() {
        return None;
    }

    let mut chars = text.chars_at(pos);

    if chars.next()? == close {
        return Some(pos);
    }

    for _ in 0..n {
        let mut step_over: usize = 0;

        loop {
            let c = chars.next()?;
            pos += 1;

            if c == open {
                step_over += 1;
            } else if c == close {
                if step_over == 0 {
                    break;
                }

                step_over = step_over.saturating_sub(1);
            }
        }
    }

    Some(pos)
}

/// Find the pair of `ch` used by `mi`/`ma`, even when the cursor is outside
/// the pair, so they behave like Vim/targets.vim text objects.
///
/// Resolution order:
/// 1. The (n-th) pair enclosing the cursor.
/// 2. A pair on the cursor's line: the nearest one after the cursor, then the
///    nearest one before it.
/// 3. The nearest pair anywhere else in the document.
///
/// Quote-like delimiters (where open == close, e.g. `"`, `'`, `` ` ``) are
/// paired left to right from the start of the cursor's line, skipping
/// backslash-escaped quotes. This way the closing quote of one string and the
/// opening quote of the next string are never treated as a pair, and a pair is
/// never formed from quotes on different lines when the cursor's line has a
/// complete pair of its own.
pub fn find_nth_pairs_pos_closest(
    _syntax: Option<&Syntax>,
    text: RopeSlice,
    ch: char,
    range: Range,
    n: usize,
) -> Result<(usize, usize)> {
    let n = n.max(1);
    let (open, close) = get_pair(ch);
    let cursor = range.cursor(text);
    if text.len_chars() == 0 {
        return Err(Error::PairNotFound);
    }

    let (start, end) = if open == close {
        find_closest_quote_pair(text, open, cursor, n)
    } else {
        find_closest_bracket_pair(text, open, close, range, cursor, n)
    }
    .ok_or(Error::PairNotFound)?;

    if range.direction() == Direction::Forward {
        Ok((start, end))
    } else {
        Ok((end, start))
    }
}

/// Returns true if the char at `pos` is preceded by an odd number of
/// backslashes, i.e. it is escaped.
fn is_escaped(text: RopeSlice, pos: usize) -> bool {
    let mut backslashes = 0;
    let mut chars = text.chars_at(pos);
    while chars.prev() == Some('\\') {
        backslashes += 1;
    }
    backslashes % 2 == 1
}

/// Pair up unescaped occurrences of `quote` in `text[from..to]`, left to right.
fn quote_pairs_in(text: RopeSlice, quote: char, from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut pending = None;
    for (offset, current) in text.slice(from..to).chars().enumerate() {
        let pos = from + offset;
        if current != quote || is_escaped(text, pos) {
            continue;
        }
        match pending.take() {
            Some(start) => pairs.push((start, pos)),
            None => pending = Some(pos),
        }
    }
    pairs
}

/// Ranking key for a candidate pair relative to the cursor: enclosing pairs
/// first (innermost first), then pairs after the cursor, then pairs before it,
/// each ordered by distance.
fn pair_rank(cursor: usize, (start, end): (usize, usize)) -> (u8, usize, usize) {
    if start <= cursor && cursor <= end {
        (0, end - start, start)
    } else if cursor < start {
        (1, start - cursor, start)
    } else {
        (2, cursor - end, start)
    }
}

/// Like [`pair_rank`] but without a forward/backward preference, used when
/// searching beyond the cursor's line.
fn pair_rank_by_distance(cursor: usize, (start, end): (usize, usize)) -> (u8, usize, u8, usize) {
    match pair_rank(cursor, (start, end)) {
        (0, span, start) => (0, span, 0, start),
        (dir, distance, start) => (1, distance, dir, start),
    }
}

/// Find the `n`-th unescaped `quote` strictly before (`Backward`) or after
/// (`Forward`) `pos`.
fn find_nth_unescaped_quote(
    text: RopeSlice,
    quote: char,
    mut pos: usize,
    n: usize,
    direction: Direction,
) -> Option<usize> {
    for _ in 0..n {
        loop {
            pos = match direction {
                Direction::Backward => pos.checked_sub(1)?,
                Direction::Forward => Some(pos + 1).filter(|&p| p < text.len_chars())?,
            };
            if text.char(pos) == quote && !is_escaped(text, pos) {
                break;
            }
        }
    }
    Some(pos)
}

fn find_closest_quote_pair(
    text: RopeSlice,
    quote: char,
    cursor: usize,
    n: usize,
) -> Option<(usize, usize)> {
    // Quotes can't be told apart from each other, so a count expands outwards
    // from the closest pair (`2mi'` on `'a 'b' c'` selects `a 'b' c`).
    let (start, end) = find_closest_quote_pair_1(text, quote, cursor)?;
    if n == 1 {
        return Some((start, end));
    }
    Some((
        find_nth_unescaped_quote(text, quote, start, n - 1, Direction::Backward)?,
        find_nth_unescaped_quote(text, quote, end, n - 1, Direction::Forward)?,
    ))
}

fn find_closest_quote_pair_1(
    text: RopeSlice,
    quote: char,
    cursor: usize,
) -> Option<(usize, usize)> {
    let line = text.char_to_line(cursor.min(text.len_chars()));
    let line_start = text.line_to_char(line);
    let line_end = if line + 1 < text.len_lines() {
        text.line_to_char(line + 1)
    } else {
        text.len_chars()
    };

    // Prefer pairs on the cursor's line. Quotes on the same line are
    // unambiguous once paired from the start of the line.
    let mut pairs = quote_pairs_in(text, quote, line_start, line_end);
    if !pairs.is_empty() {
        pairs.sort_by_key(|&pair| pair_rank(cursor, pair));
        return pairs.first().copied();
    }

    // Otherwise (e.g. a string spanning multiple lines) pair quotes across the
    // whole document and pick the nearest pair.
    let pairs = quote_pairs_in(text, quote, 0, text.len_chars());
    pairs
        .into_iter()
        .min_by_key(|&pair| pair_rank_by_distance(cursor, pair))
}

fn find_closest_bracket_pair(
    text: RopeSlice,
    open: char,
    close: char,
    range: Range,
    cursor: usize,
    n: usize,
) -> Option<(usize, usize)> {
    // An enclosing pair always wins.
    if range.to() < text.len_chars() {
        if let (Some(start), Some(end)) = (
            find_nth_open_pair(text, open, close, cursor, n),
            find_nth_close_pair(text, open, close, cursor, n),
        ) {
            return Some((start, end));
        }
    }

    let mut pairs = Vec::new();
    let mut stack = Vec::new();
    for (pos, current) in text.chars().enumerate() {
        if current == open {
            stack.push(pos);
        } else if current == close {
            if let Some(start) = stack.pop() {
                pairs.push((start, pos));
            }
        }
    }

    let (mut enclosing, mut pairs): (Vec<_>, Vec<_>) = pairs
        .into_iter()
        .partition(|&(start, end)| start <= cursor && cursor <= end);
    enclosing.sort_by_key(|&(start, end)| end - start);
    if let Some(&pair) = enclosing.get(n - 1) {
        return Some(pair);
    }
    let n = n - enclosing.len();

    let line = text.char_to_line(cursor.min(text.len_chars()));
    let on_line = |pos: usize| text.char_to_line(pos) == line;

    // Prefer pairs that start (or end) on the cursor's line.
    let mut same_line: Vec<_> = pairs
        .iter()
        .copied()
        .filter(|&(start, end)| {
            if cursor < start {
                on_line(start)
            } else {
                on_line(end)
            }
        })
        .collect();
    same_line.sort_by_key(|&pair| pair_rank(cursor, pair));
    if let Some(&pair) = same_line.get(n - 1) {
        return Some(pair);
    }

    pairs.sort_by_key(|&pair| pair_rank_by_distance(cursor, pair));
    pairs.get(n - 1).copied()
}

/// Find position of surround characters around every cursor. Returns None
/// if any positions overlap. Note that the positions are in a flat Vec.
/// Use get_surround_pos().chunks(2) to get matching pairs of surround positions.
/// `ch` can be either closing or opening pair. If `ch` is None, surround pairs
/// are automatically detected around each cursor (note that this may result
/// in them selecting different surround characters for each selection).
pub fn get_surround_pos(
    syntax: Option<&Syntax>,
    text: RopeSlice,
    selection: &Selection,
    ch: Option<char>,
    skip: usize,
) -> Result<Vec<usize>> {
    let mut change_pos = Vec::new();

    for &range in selection {
        let (open_pos, close_pos) = {
            let range_raw = match ch {
                Some(ch) => find_nth_pairs_pos(syntax, text, ch, range, skip)?,
                None => find_nth_closest_pairs_pos(syntax, text, range, skip)?,
            };
            let range = Range::new(range_raw.0, range_raw.1);
            (range.from(), range.to())
        };
        if change_pos.contains(&open_pos) || change_pos.contains(&close_pos) {
            return Err(Error::CursorOverlap);
        }
        // ensure the positions are always paired in the forward direction
        change_pos.extend_from_slice(&[open_pos.min(close_pos), close_pos.max(open_pos)]);
    }
    Ok(change_pos)
}

/// All delimiter pairs on the line of `pos`, as `(open, close)` positions:
/// brackets matched by nesting, quotes paired left to right.
fn pairs_on_line(text: RopeSlice, pos: usize) -> Vec<(usize, usize)> {
    let line = text.char_to_line(pos);
    let start = text.line_to_char(line);
    let end = start + text.line(line).len_chars();
    let mut pairs = Vec::new();
    for quote in ['"', '\'', '`'] {
        pairs.extend(quote_pairs_in(text, quote, start, end));
    }
    let mut stack: Vec<(char, usize)> = Vec::new();
    for (offset, ch) in text.slice(start..end).chars().enumerate() {
        let pos = start + offset;
        match ch {
            '(' | '[' | '{' | '<' => stack.push((ch, pos)),
            ')' | ']' | '}' | '>' => {
                let open = get_pair(ch).0;
                if let Some(i) = stack.iter().rposition(|(c, _)| *c == open) {
                    let (_, open_pos) = stack[i];
                    stack.truncate(i);
                    pairs.push((open_pos, pos));
                }
            }
            _ => {}
        }
    }
    pairs
}

/// The pair `alt-m` selects inside of, relative to `from` (the start of the
/// selection): the nearest pair on the line opening at or after `from`, else
/// the nearest one closing before it, else the innermost one around it.
pub fn find_next_pair_on_line(text: RopeSlice, from: usize) -> Option<(usize, usize)> {
    if text.len_chars() == 0 {
        return None;
    }
    let from = from.min(text.len_chars() - 1);
    let pairs = pairs_on_line(text, from);
    let right = pairs
        .iter()
        .filter(|(open, _)| *open >= from)
        .min_by_key(|(open, close)| (*open, std::cmp::Reverse(*close)));
    let left = || {
        pairs
            .iter()
            .filter(|(_, close)| *close < from)
            .max_by_key(|(open, close)| (*close, *open))
    };
    let around = || {
        pairs
            .iter()
            .filter(|(open, close)| *open < from && from <= *close)
            .min_by_key(|(open, close)| close - open)
    };
    right.or_else(left).or_else(around).copied()
}

#[cfg(test)]
mod next_pair_test {
    use super::*;

    fn inside(text: &str, from: usize) -> Option<&str> {
        let rope = ropey::Rope::from(text);
        let (open, close) = find_next_pair_on_line(rope.slice(..), from)?;
        Some(&text[open + 1..close])
    }

    #[test]
    fn next_pair_on_the_line() {
        // To the right first, outermost when two open at the same place.
        assert_eq!(inside("x = (\"hello\")", 0), Some("\"hello\""));
        assert_eq!(inside("x = (\"hello\")", 5), Some("hello"));
        // Then to the left.
        assert_eq!(inside("f(a) + b", 6), Some("a"));
        // Then around the cursor.
        assert_eq!(inside("[abc]", 2), Some("abc"));
        assert_eq!(inside("say 'it' and [x]", 0), Some("it"));
        // Only the cursor's line.
        assert_eq!(inside("no pairs\n(x)", 0), None);
        assert_eq!(inside("", 0), None);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Range;

    use ropey::Rope;
    use smallvec::SmallVec;

    #[test]
    fn test_find_nth_pairs_pos_closest_before_pair() {
        let doc = Rope::from("before (inside) after");
        let range = Range::point(0);
        assert_eq!(
            find_nth_pairs_pos_closest(None, doc.slice(..), '(', range, 1),
            Ok((7, 14))
        );
    }

    #[test]
    fn test_get_surround_pos() {
        #[rustfmt::skip]
        let (doc, selection, expectations) =
            rope_with_selections_and_expectations(
                "(some) (chars)\n(newline)",
                "_ ^  _ _ ^   _\n_    ^  _"
            );

        assert_eq!(
            get_surround_pos(None, doc.slice(..), &selection, Some('('), 1).unwrap(),
            expectations
        );
    }

    #[test]
    fn test_get_surround_pos_bail_different_surround_chars() {
        #[rustfmt::skip]
        let (doc, selection, _) =
            rope_with_selections_and_expectations(
                "[some]\n(chars)xx\n(newline)",
                "  ^   \n  ^      \n         "
            );

        assert_eq!(
            get_surround_pos(None, doc.slice(..), &selection, Some('('), 1),
            Err(Error::PairNotFound)
        );
    }

    #[test]
    fn test_get_surround_pos_bail_overlapping_surround_chars() {
        #[rustfmt::skip]
        let (doc, selection, _) =
            rope_with_selections_and_expectations(
                "[some]\n(chars)xx\n(newline)",
                "      \n       ^ \n      ^  "
            );

        assert_eq!(
            get_surround_pos(None, doc.slice(..), &selection, Some('('), 1),
            Err(Error::PairNotFound) // overlapping surround chars
        );
    }

    #[test]
    fn test_get_surround_pos_bail_cursor_overlap() {
        #[rustfmt::skip]
        let (doc, selection, _) =
            rope_with_selections_and_expectations(
                "[some]\n(chars)xx\n(newline)",
                "  ^^  \n         \n         "
            );

        assert_eq!(
            get_surround_pos(None, doc.slice(..), &selection, Some('['), 1),
            Err(Error::CursorOverlap)
        );
    }

    #[test]
    fn test_find_nth_pairs_pos_quote_success() {
        #[rustfmt::skip]
        let (doc, selection, expectations) =
            rope_with_selections_and_expectations(
                "some 'quoted text' on this 'line'\n'and this one'",
                "     _        ^  _               \n              "
            );

        assert_eq!(2, expectations.len());
        assert_eq!(
            find_nth_pairs_pos(None, doc.slice(..), '\'', selection.primary(), 1)
                .expect("find should succeed"),
            (expectations[0], expectations[1])
        )
    }

    #[test]
    fn test_find_nth_pairs_pos_nested_quote_success() {
        #[rustfmt::skip]
        let (doc, selection, expectations) =
            rope_with_selections_and_expectations(
                "some 'nested 'quoted' text' on this 'line'\n'and this one'",
                "     _           ^        _               \n              "
            );

        assert_eq!(2, expectations.len());
        assert_eq!(
            find_nth_pairs_pos(None, doc.slice(..), '\'', selection.primary(), 2)
                .expect("find should succeed"),
            (expectations[0], expectations[1])
        )
    }

    #[test]
    fn test_find_nth_pairs_pos_inside_quote_ambiguous() {
        #[rustfmt::skip]
        let (doc, selection, _) =
            rope_with_selections_and_expectations(
                "some 'nested 'quoted' text' on this 'line'\n'and this one'",
                "                    ^                     \n              "
            );

        assert_eq!(
            find_nth_pairs_pos(None, doc.slice(..), '\'', selection.primary(), 1),
            Err(Error::CursorOnAmbiguousPair)
        )
    }

    #[test]
    fn test_find_nth_closest_pairs_pos_index_range_panic() {
        #[rustfmt::skip]
        let (doc, selection, _) =
            rope_with_selections_and_expectations(
                "(a)c)",
                "^^^^^"
            );

        assert_eq!(
            find_nth_closest_pairs_pos(None, doc.slice(..), selection.primary(), 1),
            Err(Error::PairNotFound)
        )
    }

    // Create a Rope and a matching Selection using a specification language.
    // ^ is a single-point selection.
    // _ is an expected index. These are returned as a Vec<usize> for use in assertions.
    fn rope_with_selections_and_expectations(
        text: &str,
        spec: &str,
    ) -> (Rope, Selection, Vec<usize>) {
        if text.len() != spec.len() {
            panic!("specification must match text length -- are newlines aligned?");
        }

        let rope = Rope::from(text);

        let selections: SmallVec<[Range; 1]> = spec
            .match_indices('^')
            .map(|(i, _)| Range::point(i))
            .collect();

        let expectations: Vec<usize> = spec.match_indices('_').map(|(i, _)| i).collect();

        (rope, Selection::new(selections, 0), expectations)
    }
}
