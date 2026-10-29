# Vendored crates

These are local copies of crates from crates.io with changes to improve editing
performance in large files. They replace the crates.io versions through
`[patch.crates-io]` in the workspace `Cargo.toml`.

| Crate | Version |
|---|---|
| `tree-house` | 0.4.0 |
| `tree-house-bindings` | 0.3.2 |

## Why

After every edit, `tree-house` used to re-run the injection query and the
locals query over the **whole** document. Tree-sitter's incremental re-parse is
cheap, but those two queries made every keystroke cost time proportional to the
file size. Profiling a 5,600-line Lua file showed that almost all of the work
of updating the syntax tree after a keystroke went to these two queries, and
very little to the re-parse.

## What changed

### `tree-house-bindings`

New bindings for existing tree-sitter C functions:

- `Tree::changed_ranges` (`ts_tree_get_changed_ranges`)
- `InactiveQueryCursor::set_max_start_depth` (`ts_query_cursor_set_max_start_depth`).
  Cursors are cached and reused, so `InactiveQueryCursor::new` resets the limit.
- `Query::is_pattern_rooted` (`ts_query_is_pattern_rooted`)

### `tree-house`

After an edit, the root layer re-runs the injection and locals queries only
over the *changed regions*. These regions cover:

- the ranges tree-sitter reports as structurally changed,
- the edited text,
- old injections touched by an edit or overlapping a region,

widened to whole children of the root node. Every match rooted below the root
node therefore lies entirely inside or entirely outside a region.

- **Injections:** old injections outside the regions are kept as they are and
  reuse their layers exactly as a full run would. Inside the regions, matches
  go through the same per-match code as before.
- **Locals:** the scope and definition captures from the last run are cached.
  The new capture stream is the old captures outside the regions plus fresh
  captures inside them, with root-node captures merged in. The scope tree is
  rebuilt from that stream by the same loop as before.

The original full-document code path is used whenever the incremental one
can't guarantee the same result:

- the first parse, or a layer that is not the root layer,
- injection queries with `injection.combined` patterns,
- patterns with several root nodes,
- patterns that match the root node itself (for locals: patterns that capture
  anything other than the root node),
- the cached locals captures were collected with different highlight scopes,
  for example before a theme change.

### Faster query compilation (`tree-house`)

Compiling a language's queries is most of the editor's startup: about 100 ms
for Rust, then as much again for `rust-format-args-macro`, which Rust files
inject into `format!` and friends and whose highlights and locals are Rust's.

- `LanguageConfig::new` compiles the highlights, injections and locals
  queries on threads of their own at the same time (for queries over 4 KiB;
  smaller ones aren't worth a thread), then gives the threads' freed memory
  back with `malloc_trim` (glibc keeps it otherwise).
- Languages with the same grammar and the same highlights and locals share
  one compiled `HighlightQuery` (an `Arc`), so `rust-format-args-macro` only
  compiles its injections.

Startup on a 257 KB Rust file went from 341 ms to 273 ms.

## Testing

`helix-core/tests/incremental_syntax.rs` applies random edits to documents in
several languages. It checks that the layers, syntax trees, locals and
highlights after each edit are identical to the result of the original
full-document queries (`tree_house::set_incremental_injections(false)`).
`identical_highlight_queries_are_compiled_once` checks the sharing.
