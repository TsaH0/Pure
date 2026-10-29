//! API 0.5 `syntax`: tree-sitter queries and tree walks over the syntax
//! tree in a document's snapshot.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use helix_core::tree_sitter::query::{InvalidPredicateError, UserPredicate};
use helix_core::tree_sitter::{Grammar, InactiveQueryCursor, Node, Query, Tree};
use helix_core::Rope;

/// Most matches one query returns.
pub const MAX_MATCHES: usize = 10_000;
/// Longest query source accepted.
pub const MAX_QUERY_LEN: usize = 256 * 1024;
/// Compiled queries kept.
const CACHED_QUERIES: usize = 32;

/// A node as plugins see it: kind and char range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxNode {
    pub kind: String,
    pub named: bool,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryMatch {
    pub pattern: u32,
    pub captures: Vec<(String, SyntaxNode)>,
}

/// Recently compiled queries by grammar and source, newest last.
static QUERIES: Mutex<VecDeque<(Grammar, String, Arc<Query>)>> = Mutex::new(VecDeque::new());

fn compile(grammar: Grammar, source: &str) -> Result<Arc<Query>, String> {
    if source.len() > MAX_QUERY_LEN {
        return Err(format!("query too long: over {MAX_QUERY_LEN} bytes"));
    }
    let mut queries = QUERIES.lock().unwrap();
    if let Some(i) = queries
        .iter()
        .position(|(other, text, _)| *other == grammar && text == source)
    {
        let entry = queries.remove(i).unwrap();
        let query = entry.2.clone();
        queries.push_back(entry);
        return Ok(query);
    }
    drop(queries);
    // Properties (`#set!`) mean nothing here; other predicates are errors.
    let query = Query::new(grammar, source, |_, predicate| match predicate {
        UserPredicate::SetProperty { .. } => Ok(()),
        predicate => Err(InvalidPredicateError::unknown(predicate)),
    })
    .map(Arc::new)
    .map_err(|err| format!("invalid query: {err}"))?;
    let mut queries = QUERIES.lock().unwrap();
    if queries.len() >= CACHED_QUERIES {
        queries.pop_front();
    }
    queries.push_back((grammar, source.to_owned(), query.clone()));
    Ok(query)
}

fn char_to_byte(text: &Rope, pos: u32) -> Result<u32, String> {
    let pos = pos as usize;
    if pos > text.len_chars() {
        return Err(format!(
            "position {pos} is past the end of a document of {} chars",
            text.len_chars()
        ));
    }
    Ok(text.char_to_byte(pos) as u32)
}

fn node(text: &Rope, node: &Node) -> SyntaxNode {
    let len = text.len_bytes();
    let char_at = |byte: u32| text.byte_to_char((byte as usize).min(len)) as u32;
    SyntaxNode {
        kind: node.kind().to_owned(),
        named: node.is_named(),
        start: char_at(node.start_byte()),
        end: char_at(node.end_byte()),
    }
}

pub fn query(
    tree: &Tree,
    text: &Rope,
    source: &str,
    range: Option<(u32, u32)>,
) -> Result<Vec<QueryMatch>, String> {
    let root = tree.root_node();
    let query = compile(root.grammar(), source)?;
    let bytes = match range {
        Some((from, to)) if from > to => {
            return Err(format!("invalid range {from}..{to}"));
        }
        Some((from, to)) => char_to_byte(text, from)?..char_to_byte(text, to)?,
        None => 0..u32::MAX,
    };
    let mut cursor = InactiveQueryCursor::new(bytes, MAX_MATCHES as u32).execute_query(
        &query,
        &root,
        text.slice(..),
    );
    let mut matches = Vec::new();
    while let Some(found) = cursor.next_match() {
        if matches.len() == MAX_MATCHES {
            break;
        }
        matches.push(QueryMatch {
            pattern: found.pattern().idx() as u32,
            captures: found
                .matched_nodes()
                .map(|matched| {
                    (
                        query.capture_name(matched.capture).to_owned(),
                        node(text, &matched.node),
                    )
                })
                .collect(),
        });
    }
    Ok(matches)
}

/// The query file `name` for `language`, as the editor ships it.
pub fn query_file(language: &str, name: &str) -> Option<String> {
    let plain = |s: &str, extra: &[char]| {
        !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || extra.contains(&c))
    };
    // Never a path: only names like `rust` and `textobjects.scm`.
    let name_ok = name
        .strip_suffix(".scm")
        .is_some_and(|stem| plain(stem, &['_', '-']));
    if !name_ok || !plain(language, &['_', '-']) {
        return None;
    }
    let text = helix_core::syntax::read_query(language, name);
    (!text.trim().is_empty()).then_some(text)
}

pub fn node_at(tree: &Tree, text: &Rope, from: u32, to: u32, named: bool) -> Option<SyntaxNode> {
    if from > to {
        return None;
    }
    let (from, to) = (char_to_byte(text, from).ok()?, char_to_byte(text, to).ok()?);
    let root = tree.root_node();
    let found = if named {
        root.named_descendant_for_byte_range(from, to)
    } else {
        root.descendant_for_byte_range(from, to)
    }?;
    Some(node(text, &found))
}

pub fn root(tree: &Tree, text: &Rope) -> SyntaxNode {
    node(text, &tree.root_node())
}

/// The tree's node with `wanted`'s kind and range.
fn find<'tree>(tree: &'tree Tree, text: &Rope, wanted: &SyntaxNode) -> Result<Node<'tree>, String> {
    let gone = || {
        format!(
            "no {} node at {}..{}: the document changed",
            wanted.kind, wanted.start, wanted.end
        )
    };
    if wanted.start > wanted.end {
        return Err(gone());
    }
    let from = char_to_byte(text, wanted.start).map_err(|_| gone())?;
    let to = char_to_byte(text, wanted.end).map_err(|_| gone())?;
    let mut current = tree.root_node().descendant_for_byte_range(from, to);
    while let Some(candidate) = current {
        if candidate.byte_range() != (from..to) {
            break;
        }
        if candidate.kind() == wanted.kind && candidate.is_named() == wanted.named {
            return Ok(candidate);
        }
        current = candidate.parent();
    }
    Err(gone())
}

pub fn parent(tree: &Tree, text: &Rope, of: &SyntaxNode) -> Result<Option<SyntaxNode>, String> {
    Ok(find(tree, text, of)?
        .parent()
        .map(|parent| node(text, &parent)))
}

pub fn children(
    tree: &Tree,
    text: &Rope,
    of: &SyntaxNode,
    named: bool,
) -> Result<Vec<SyntaxNode>, String> {
    Ok(find(tree, text, of)?
        .children()
        .filter(|child| !named || child.is_named())
        .map(|child| node(text, &child))
        .collect())
}

pub fn sibling(
    tree: &Tree,
    text: &Rope,
    of: &SyntaxNode,
    named: bool,
    next: bool,
) -> Result<Option<SyntaxNode>, String> {
    let found = find(tree, text, of)?;
    let sibling = match (next, named) {
        (true, true) => found.next_named_sibling(),
        (true, false) => found.next_sibling(),
        (false, true) => found.prev_named_sibling(),
        (false, false) => found.prev_sibling(),
    };
    Ok(sibling.map(|sibling| node(text, &sibling)))
}

pub fn sexp(tree: &Tree, text: &Rope, of: &SyntaxNode) -> Result<String, String> {
    let mut out = String::new();
    helix_core::syntax::pretty_print_tree(&mut out, find(tree, text, of)?)
        .map_err(|err| err.to_string())?;
    Ok(out)
}
