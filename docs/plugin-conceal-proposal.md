# Conceal decorations

Status: **implemented** (2026-10-20). The owner's answer to the open
questions below: "whatever render-markdown.nvim does" (see "Decisions").
What changed from this proposal: the API names `set-conceals` and
`set-render-modes`; the replacement is drawn as a document grapheme (not
virtual text) so selections and clicks act on the hidden range; only the
*primary* cursor's line is raw.

## What it is for

Plugins can already draw *other* characters over the text (overlays) and
add virtual text, but every document character still takes at least one
cell. Conceal hides characters entirely, so a plugin can show:

- Markdown `**bold**` as **bold**, `` `code` `` without backticks, and
  `[text](https://…)` as just `text` (render-markdown.nvim, Neovim's
  `conceallevel=2`);
- folded JSON/YAML values, long URLs or hashes as `…`;
- LaTeX `\alpha` as `α` (conceal plus a one-cell replacement).

## API (API 0.4 `decorations`, an import, so 0.3 could get it too)

```wit
record conceal {
  %from: u32,          // char offsets, grapheme-aligned (checked)
  to: u32,
  /// Drawn instead of the hidden text: empty (hide), or one grapheme
  /// (e.g. `…`, `α`). Never longer: the point is to take less room.
  replacement: string,
  scope: string,       // style of the replacement
}
set-conceals: func(doc: document-id, layer: string, conceals: list<conceal>) -> result<_, string>;
```

- Ranges are stored like highlights (`PluginDecorations`, mapped through
  edits; a range whose text is edited is dropped until the plugin sets the
  layer again).
- `set-raw-on-cursor-line` applies: by default **concealed text is shown
  raw on the line with the primary cursor** (and on lines with any
  selection's head), so editing never happens in hidden text. A layer can
  opt out (`raw-on-cursor-line = false`) only for replacements that don't
  hide editable content.
- Limits: as other layers (100,000 per layer); ranges may not overlap
  within a layer; across layers the first wins.

## Rendering: `DocumentFormatter`

The formatter (`helix-core/src/doc_formatter.rs`) yields graphemes with
their document position. Conceal is a third `TextAnnotations` kind next to
inline annotations and overlays:

- `TextAnnotations::add_conceals(&[Conceal], Option<Highlight>)`, sorted
  and queried like overlays (`conceal_at(char_pos)` with a moving cursor,
  so a frame stays O(visible text)).
- In `advance_grapheme`: when a document grapheme starts a concealed range,
  emit the replacement (if any) as `GraphemeSource::VirtualText` with width
  of the replacement, then **skip** the graphemes up to `to`, advancing
  `char_pos` but emitting nothing. Newlines are never concealed (a range
  ending in or crossing a line break is clamped), so line structure, soft
  wrap and line numbers are unchanged.
- Soft wrap measures what is shown (concealed text takes no columns).

Because the formatter is also what maps positions to screen cells and back,
every consumer stays consistent if it goes through it:

| Consumer | What changes |
| --- | --- |
| `render_document` | draws the replacement, nothing for hidden graphemes |
| `visual_offset_from_block` / `pos_at_visual_coords` / `char_idx_at_visual_offset` (cursor placement, scrolling, mouse clicks) | they already iterate the formatter; hidden graphemes have no cell, so a click on a replacement maps to the range's start |
| Cursor drawing | the cursor's line is raw (above), so the cursor is never inside hidden text; secondary cursors on other lines are drawn on the replacement (or the next visible cell) |
| Selections | highlighted over what is shown; hidden text inside a selection is still selected (copy, delete act on the real text) |
| Search, `:s`, textobjects, motions | unchanged: they work on the text, not the screen. `w`/`e` may land in hidden text on other lines; that's fine, the line turns raw only when the cursor is on it |
| Diagnostics, highlights, overlays inside hidden text | not drawn (their cells don't exist); diagnostics still show in the gutter and pickers |
| Horizontal scrolling (`soft-wrap` off) | the offset counts shown columns |

## Decisions (render-markdown.nvim's behaviour)

1. Raw text on the whole primary cursor line (`anti_conceal`).
2. Lines with secondary selections stay rendered.
3. Insert (and select) mode: nothing rendered, per layer
   (`set-render-modes [normal]`, like `render_modes = { 'n', 'c', 't' }`).
4. Select mode: as 3.
5. Mouse: hidden text has no cell; a click on a replacement lands at the
   range's start.

## Behaviour that was open (answered above)

1. Raw cursor line: per line (proposed), or only the concealed range under
   the cursor (Neovim's `concealcursor` variants)? Per line is simpler and
   matches render-markdown.nvim.
2. Lines with secondary selections: raw too (proposed: yes, any selection
   head), or only the primary?
3. Insert mode: raw on the cursor line only (proposed) or the whole
   document (simplest, like `conceallevel` + `concealcursor=n`)?
4. Should `select` mode reveal everything it covers?
5. Mouse: click on a replacement → start of the range (proposed).

## Tests to write with it

- Formatter: grapheme-aligned ranges, multi-codepoint graphemes (emoji
  ZWJ, combining marks), CRLF, tabs right after hidden text, wide chars as
  replacements, ranges at line ends, empty documents.
- Position mapping round trips: `pos → screen → pos` for every position on
  lines with conceals, with and without soft wrap.
- Mouse: clicks on, before and after replacements.
- Rendering snapshots: Markdown bold/code/links concealed; the cursor line
  raw; two views of one document with different cursor lines.
- Performance: a 10,000-line Markdown file with conceals on every line:
  frame time within noise of no conceals (only visible lines are looked
  at), and nothing when no plugin sets conceals.

## Size

`helix-core` (formatter, annotations, ~250 lines + tests), `helix-view`
(storage and mapping, ~120), `helix-term` (raw cursor line, ~40), host and
WIT (~80), the markdown example (~60). The formatter change touches code
upstream Helix changes too; keep it isolated in `text_annotations.rs` and
one branch of `advance_grapheme` to ease merges.
