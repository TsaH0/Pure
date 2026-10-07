use std::{
    borrow::Cow,
    ops::{Index, IndexMut},
    sync::Arc,
};

use hashbrown::HashMap;
use kstring::KString;
use ropey::RopeSlice;
use tree_sitter::{Capture, InactiveQueryCursor, MatchedNodeIdx, Pattern, QueryMatch, Tree};

use crate::highlighter::Highlight;
use crate::injections_query::{changed_regions, ranges_overlap};
use crate::{
    InjectionsQuery, LanguageConfig, LanguageLoader, Layer, Range, Syntax, TREE_SITTER_MATCH_LIMIT,
};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct Scope(u32);

impl Scope {
    const ROOT: Scope = Scope(0);
    fn idx(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone)]
pub struct Locals {
    scopes: Vec<ScopeData>,
}

impl Default for Locals {
    fn default() -> Self {
        let mut scopes = Vec::with_capacity(4);
        scopes.push(ScopeData {
            definitions: HashMap::new(),
            range: 0..u32::MAX,
            inherit: false,
            children: Vec::new(),
            parent: None,
        });

        Self { scopes }
    }
}

impl Locals {
    fn push(&mut self, scope: ScopeData) -> Scope {
        let new_scope_id = Scope(self.scopes.len() as u32);
        let parent = scope
            .parent
            .expect("push cannot be used for the root layer");
        self[parent].children.push(new_scope_id);
        self.scopes.push(scope);
        new_scope_id
    }

    pub fn lookup_reference(&self, mut scope: Scope, name: &str) -> Option<&Definition> {
        loop {
            let scope_data = &self[scope];
            if let Some(def) = scope_data.definitions.get(name) {
                return Some(def);
            }
            if !scope_data.inherit {
                break;
            }
            scope = scope_data.parent?;
        }

        None
    }

    pub fn scope_cursor(&self, pos: u32) -> ScopeCursor<'_> {
        let mut scope = Scope::ROOT;
        let mut scope_stack = Vec::with_capacity(8);
        loop {
            let scope_data = &self[scope];
            let child_idx = scope_data
                .children
                .partition_point(|&child| self[child].range.end < pos);
            scope_stack.push((scope, child_idx as u32));
            let Some(&child) = scope_data.children.get(child_idx) else {
                break;
            };
            if pos < self[child].range.start {
                break;
            }
            scope = child;
        }
        ScopeCursor {
            locals: self,
            scope_stack,
        }
    }
}

impl Locals {
    /// A canonical textual representation of the scope tree, used to compare locals in tests.
    #[doc(hidden)]
    pub fn debug_canonical(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for (i, scope) in self.scopes.iter().enumerate() {
            let mut definitions: Vec<_> = scope
                .definitions
                .iter()
                .map(|(name, def)| format!("{name}={:?}@{:?}", def.capture, def.range))
                .collect();
            definitions.sort();
            let _ = writeln!(
                out,
                "{i}: {:?} inherit={} parent={:?} children={:?} defs={definitions:?}",
                scope.range, scope.inherit, scope.parent, scope.children
            );
        }
        out
    }
}

impl Index<Scope> for Locals {
    type Output = ScopeData;

    fn index(&self, scope: Scope) -> &Self::Output {
        &self.scopes[scope.idx()]
    }
}

impl IndexMut<Scope> for Locals {
    fn index_mut(&mut self, scope: Scope) -> &mut Self::Output {
        &mut self.scopes[scope.idx()]
    }
}

#[derive(Debug)]
pub struct ScopeCursor<'a> {
    pub locals: &'a Locals,
    scope_stack: Vec<(Scope, u32)>,
}

impl ScopeCursor<'_> {
    pub fn advance(&mut self, to: u32) -> Scope {
        let (mut active_scope, mut child_idx) = self.scope_stack.pop().unwrap();
        loop {
            let scope_data = &self.locals[active_scope];
            if to < scope_data.range.end {
                break;
            }
            (active_scope, child_idx) = self.scope_stack.pop().unwrap();
            child_idx += 1;
        }
        'outer: loop {
            let scope_data = &self.locals[active_scope];
            loop {
                let Some(&child) = scope_data.children.get(child_idx as usize) else {
                    break 'outer;
                };
                if self.locals[child].range.start > to {
                    break 'outer;
                }
                if to < self.locals[child].range.end {
                    self.scope_stack.push((active_scope, child_idx));
                    active_scope = child;
                    child_idx = 0;
                    break;
                }
                child_idx += 1;
            }
        }
        self.scope_stack.push((active_scope, child_idx));
        active_scope
    }

    pub fn current_scope(&self) -> Scope {
        // The root scope is always active so `scope_stack` is never empty.
        self.scope_stack.last().unwrap().0
    }
}

#[derive(Debug, Clone)]
pub struct Definition {
    pub capture: Capture,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct ScopeData {
    definitions: HashMap<KString, Definition>,
    range: Range,
    inherit: bool,
    /// A list of sorted, non-overlapping child scopes.
    ///
    /// See the docs of the `Locals` type: locals information is laid out like a tree - similar
    /// to injections - per injection layer.
    children: Vec<Scope>,
    parent: Option<Scope>,
}

/// A `@local.scope` or `@local.definition.*` capture of the locals query.
///
/// The captures of the last run of the locals query are kept so that the query only needs to
/// be re-run over the changed parts of the document after an edit.
#[derive(Debug, Clone)]
pub(crate) struct LocalCapture {
    range: Range,
    pattern: Pattern,
    kind: LocalCaptureKind,
}

#[derive(Debug, Clone)]
enum LocalCaptureKind {
    Scope,
    Definition { capture: Capture, text: KString },
}

/// The cached captures of the locals query for a layer.
#[derive(Debug, Clone)]
pub(crate) struct LocalCaptures {
    /// Captures of nodes other than the root node, in the order returned by the query.
    captures: Vec<LocalCapture>,
    /// The definition captures the captures were collected with. These change when the
    /// highlight scopes are reconfigured (for example when the theme changes).
    definition_captures: Arc<HashMap<Capture, Highlight>>,
}

impl LocalCapture {
    fn new(
        query_match: &QueryMatch<'_, '_>,
        node_idx: MatchedNodeIdx,
        injection_query: &InjectionsQuery,
        definition_captures: &HashMap<Capture, Highlight>,
        source: RopeSlice<'_>,
    ) -> Option<Self> {
        let matched_node = query_match.matched_node(node_idx);
        let range = matched_node.node.byte_range();
        let capture = matched_node.capture;
        let kind = if Some(capture) == injection_query.local_scope_capture {
            LocalCaptureKind::Scope
        } else if definition_captures.contains_key(&capture) {
            let text = match source
                .byte_slice(range.start as usize..range.end as usize)
                .into()
            {
                Cow::Borrowed(inner) => KString::from_ref(inner),
                Cow::Owned(inner) => KString::from_string(inner),
            };
            LocalCaptureKind::Definition { capture, text }
        } else {
            // NOTE: `local.reference` captures are handled by the highlighter and are not
            // considered during parsing. Other captures do not affect the locals (skipping
            // them is fine since captures are visited in order of their start position).
            return None;
        };
        Some(LocalCapture {
            range,
            pattern: query_match.pattern(),
            kind,
        })
    }
}

/// Builds the scope tree from the captures of the locals query (in query order).
fn build_locals<'a>(
    captures: impl Iterator<Item = &'a LocalCapture>,
    injection_query: &InjectionsQuery,
) -> Locals {
    let mut locals = Locals::default();
    let mut scope = Scope::ROOT;
    for local_capture in captures {
        let range = &local_capture.range;
        while range.start >= locals[scope].range.end {
            scope = locals[scope].parent.expect("root node covers entire range");
        }
        match &local_capture.kind {
            LocalCaptureKind::Scope => {
                scope = locals.push(ScopeData {
                    definitions: HashMap::new(),
                    range: range.clone(),
                    inherit: !injection_query
                        .not_scope_inherits
                        .contains(&local_capture.pattern),
                    children: Vec::new(),
                    parent: Some(scope),
                });
            }
            LocalCaptureKind::Definition { capture, text } => {
                locals[scope].definitions.insert(
                    text.clone(),
                    Definition {
                        capture: *capture,
                        range: range.clone(),
                    },
                );
            }
        }
    }
    locals
}

/// Returns the position at which a capture of the root node is returned by the query among
/// the (sorted) `captures` of other nodes.
///
/// Tree-sitter returns captures ordered by their start byte. Captures starting at the same
/// byte are ordered by their pattern index, and captures of the same pattern in the order in
/// which the matches started: the root node is visited first.
fn root_capture_position(captures: &[LocalCapture], root_capture: &LocalCapture) -> usize {
    captures.partition_point(|capture| {
        capture.range.start < root_capture.range.start
            || (capture.range.start == root_capture.range.start
                && capture.pattern < root_capture.pattern)
    })
}

/// Merges the captures of the root node into the (sorted) captures of the other nodes, in the
/// order in which the query returns them.
fn merged_captures<'a>(
    captures: &'a [LocalCapture],
    root_captures: &'a [LocalCapture],
) -> impl Iterator<Item = &'a LocalCapture> {
    let mut root_captures = root_captures
        .iter()
        .map(|root_capture| (root_capture_position(captures, root_capture), root_capture))
        .collect::<Vec<_>>()
        .into_iter()
        .peekable();
    let mut captures = captures.iter().enumerate().peekable();
    std::iter::from_fn(move || {
        let next = captures.peek().map_or(usize::MAX, |(i, _)| *i);
        if let Some((_, root_capture)) = root_captures.next_if(|(position, _)| *position <= next) {
            return Some(root_capture);
        }
        captures.next().map(|(_, capture)| capture)
    })
}

impl Syntax {
    pub(crate) fn run_local_query(
        &mut self,
        layer: Layer,
        edits: &[tree_sitter::InputEdit],
        old_tree: Option<&Tree>,
        source: RopeSlice<'_>,
        loader: &impl LanguageLoader,
    ) {
        let layer_data = &mut self.layer_mut(layer);
        let Some(LanguageConfig {
            ref injection_query,
            ..
        }) = loader.get_config(layer_data.language)
        else {
            return;
        };
        let definition_captures = injection_query.local_definition_captures.load_full();
        if definition_captures.is_empty() {
            layer_data.local_captures = None;
            return;
        }

        let tree = layer_data.parse_tree.as_ref().unwrap();
        let root = tree.root_node();
        // Only the root layer re-runs the query over the changed parts of the document,
        // injected layers are usually small.
        let cacheable = layer_data.parent.is_none() && injection_query.local_incremental;

        // Captures of patterns which match the root node itself. Patterns matching the root
        // node may only capture the root node, otherwise their captures could span the whole
        // document.
        let mut root_captures = Vec::new();
        let mut root_only = cacheable;
        if cacheable {
            let mut cursor = InactiveQueryCursor::new(0..u32::MAX, TREE_SITTER_MATCH_LIMIT);
            cursor.set_max_start_depth(0);
            let mut cursor = cursor.execute_query(&injection_query.local_query, &root, source);
            while let Some((query_match, node_idx)) = cursor.next_matched_node() {
                if query_match.matched_node(node_idx).node != root {
                    root_only = false;
                    break;
                }
                root_captures.extend(LocalCapture::new(
                    &query_match,
                    node_idx,
                    injection_query,
                    &definition_captures,
                    source,
                ));
            }
        }

        let incremental = match (old_tree, layer_data.local_captures.take()) {
            (Some(old_tree), Some(old_captures))
                if !edits.is_empty()
                    && root_only
                    && Arc::ptr_eq(&old_captures.definition_captures, &definition_captures)
                    && crate::injections_query::incremental_injections() =>
            {
                Some((old_tree, old_captures))
            }
            _ => None,
        };

        let captures = match incremental {
            Some((old_tree, old_captures)) => {
                let (regions, _) = changed_regions(old_tree, tree, edits, &[], &[]);
                let mut captures = Vec::with_capacity(old_captures.captures.len());
                let mut old_captures = map_local_captures(old_captures.captures, edits)
                    .filter(|capture| {
                        let i = regions.partition_point(|region| region.end < capture.range.start);
                        !regions
                            .get(i)
                            .is_some_and(|region| ranges_overlap(region, &capture.range))
                    })
                    .peekable();
                for region in &regions {
                    while let Some(capture) =
                        old_captures.next_if(|capture| capture.range.end <= region.start)
                    {
                        captures.push(capture);
                    }
                    let mut cursor =
                        InactiveQueryCursor::new(region.clone(), TREE_SITTER_MATCH_LIMIT)
                            .execute_query(&injection_query.local_query, &root, source);
                    while let Some((query_match, node_idx)) = cursor.next_matched_node() {
                        let node = &query_match.matched_node(node_idx).node;
                        let range = node.byte_range();
                        // the root node is handled separately and nodes which merely touch the
                        // region are covered by the kept captures
                        if *node == root || range.start < region.start || range.end > region.end {
                            continue;
                        }
                        captures.extend(LocalCapture::new(
                            &query_match,
                            node_idx,
                            injection_query,
                            &definition_captures,
                            source,
                        ));
                    }
                }
                captures.extend(old_captures);
                captures
            }
            None => {
                let mut cursor = InactiveQueryCursor::new(0..u32::MAX, TREE_SITTER_MATCH_LIMIT)
                    .execute_query(&injection_query.local_query, &root, source);
                // all captures, flagged whether they capture the root node
                let mut all = Vec::new();
                while let Some((query_match, node_idx)) = cursor.next_matched_node() {
                    let is_root = query_match.matched_node(node_idx).node == root;
                    all.extend(
                        LocalCapture::new(
                            &query_match,
                            node_idx,
                            injection_query,
                            &definition_captures,
                            source,
                        )
                        .map(|capture| (capture, is_root)),
                    );
                }
                // Build the locals from the exact order in which the query returned the
                // captures.
                layer_data.locals =
                    build_locals(all.iter().map(|(capture, _)| capture), injection_query);

                // Only keep the captures for later (incremental) updates if the captures of
                // the root node can be placed correctly among the other captures.
                let mut root_positions = Vec::new();
                let mut captures = Vec::with_capacity(all.len());
                for (capture, is_root) in all {
                    if is_root {
                        root_positions.push(captures.len());
                    } else {
                        captures.push(capture);
                    }
                }
                let consistent = root_only
                    && root_positions.len() == root_captures.len()
                    && root_positions.iter().zip(&root_captures).all(
                        |(&position, root_capture)| {
                            position == root_capture_position(&captures, root_capture)
                        },
                    );
                layer_data.local_captures = consistent.then_some(LocalCaptures {
                    captures,
                    definition_captures,
                });
                return;
            }
        };

        layer_data.locals =
            build_locals(merged_captures(&captures, &root_captures), injection_query);
        layer_data.local_captures = Some(LocalCaptures {
            captures,
            definition_captures,
        });
    }
}

/// Maps the (sorted) captures through the edits, dropping every capture touched by an edit.
/// Edits are given in the coordinates of the old text (each edit assumes that the previous
/// edits have not been applied yet).
fn map_local_captures(
    captures: Vec<LocalCapture>,
    edits: &[tree_sitter::InputEdit],
) -> impl Iterator<Item = LocalCapture> + '_ {
    let mut offset: i64 = 0;
    let mut edits = edits.iter().peekable();
    captures.into_iter().filter_map(move |mut capture| {
        while let Some(edit) = edits.next_if(|edit| edit.old_end_byte < capture.range.start) {
            offset += edit.offset() as i64;
        }
        if edits
            .peek()
            .is_some_and(|edit| edit.start_byte <= capture.range.end)
        {
            return None;
        }
        capture.range = (capture.range.start as i64 + offset) as u32
            ..(capture.range.end as i64 + offset) as u32;
        Some(capture)
    })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn cursor() {
        let mut locals = Locals::default();
        let scope1 = locals.push(ScopeData {
            definitions: Default::default(),
            range: 5..105,
            inherit: true,
            // NOTE: the subsequent call to `push` below will add scope2 to scope1's children.
            children: Default::default(),
            parent: Some(Scope::ROOT),
        });
        let scope2 = locals.push(ScopeData {
            definitions: Default::default(),
            range: 10..100,
            inherit: true,
            children: Default::default(),
            parent: Some(scope1),
        });

        let mut cursor = locals.scope_cursor(0);
        assert_eq!(cursor.current_scope(), Scope::ROOT);
        assert_eq!(cursor.advance(3), Scope::ROOT);
        assert_eq!(cursor.advance(5), scope1);
        assert_eq!(cursor.advance(8), scope1);
        assert_eq!(cursor.advance(10), scope2);
        assert_eq!(cursor.advance(50), scope2);
        assert_eq!(cursor.advance(100), scope1);
        assert_eq!(cursor.advance(105), Scope::ROOT);
        assert_eq!(cursor.advance(110), Scope::ROOT);

        let mut cursor = locals.scope_cursor(8);
        assert_eq!(cursor.current_scope(), scope1);
        assert_eq!(cursor.advance(10), scope2);
        assert_eq!(cursor.advance(100), scope1);
        assert_eq!(cursor.advance(110), Scope::ROOT);

        let mut cursor = locals.scope_cursor(10);
        assert_eq!(cursor.current_scope(), scope2);
        assert_eq!(cursor.advance(100), scope1);
        assert_eq!(cursor.advance(110), Scope::ROOT);
    }
}
