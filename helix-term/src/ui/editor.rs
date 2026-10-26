use crate::{
    commands::{self, OnKeyCallback, OnKeyCallbackKind},
    compositor::{Component, Context, Event, EventResult},
    events::{OnModeSwitch, PostCommand},
    handlers::completion::CompletionItem,
    key,
    keymap::{KeyTrie, KeymapResult, Keymaps},
    ui::{
        document::{render_document, DocumentHighlighter, LinePos, TextRenderer},
        statusline,
        text_decorations::{self, Decoration, DecorationManager, InlineDiagnostics},
        Completion, ProgressSpinners,
    },
};

use helix_core::{
    diagnostic::NumberOrString,
    graphemes::{next_grapheme_boundary, prev_grapheme_boundary},
    movement::Direction,
    syntax::{self, OverlayHighlights},
    text_annotations::TextAnnotations,
    unicode::width::UnicodeWidthStr,
    visual_offset_from_block, Change, Position, Range, Selection, Transaction,
};
use helix_view::{
    annotations::diagnostics::DiagnosticFilter,
    document::{Mode, SCRATCH_BUFFER_NAME},
    editor::{CompleteAction, CursorShapeConfig},
    graphics::{Color, CursorKind, Modifier, Rect, Style},
    input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    keyboard::{KeyCode, KeyModifiers},
    Document, Editor, Theme, View,
};
use std::{mem::take, num::NonZeroUsize, ops, path::PathBuf, rc::Rc};

use tui::{buffer::Buffer as Surface, text::Span};

pub struct EditorView {
    pub keymaps: Keymaps,
    on_next_key: Option<(OnKeyCallback, OnKeyCallbackKind)>,
    pseudo_pending: Vec<KeyEvent>,
    pub(crate) last_insert: (commands::MappableCommand, Vec<InsertEvent>),
    pub(crate) completion: Option<Completion>,
    spinners: ProgressSpinners,
    /// Tracks if the terminal window is focused by reaction to terminal focus events
    terminal_focused: bool,
    /// Where plugin panels were drawn last, for keys and mouse clicks.
    panel_areas: Vec<(helix_view::sidebar::Dock, Rect)>,
}

#[derive(Debug, Clone)]
pub enum InsertEvent {
    Key(KeyEvent),
    CompletionApply {
        trigger_offset: usize,
        changes: Vec<Change>,
    },
    TriggerCompletion,
    RequestCompletion,
}

impl EditorView {
    pub fn new(keymaps: Keymaps) -> Self {
        Self {
            keymaps,
            on_next_key: None,
            pseudo_pending: Vec::new(),
            last_insert: (commands::MappableCommand::normal_mode, Vec::new()),
            completion: None,
            spinners: ProgressSpinners::default(),
            terminal_focused: true,
            panel_areas: Vec::new(),
        }
    }

    pub fn spinners_mut(&mut self) -> &mut ProgressSpinners {
        &mut self.spinners
    }

    pub fn render_view(
        &self,
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        is_focused: bool,
    ) {
        let inner = view.inner_area(doc);
        let area = view.area;
        let theme = &editor.theme;
        let config = editor.config();
        let loader = editor.syn_loader.load();

        let view_offset = doc.view_offset(view.id);

        if !doc.plugin_decorations.is_empty() {
            doc.plugin_decorations.set_render_mode(editor.mode());
        }
        // `:s` being typed: the substitution shown in place.
        let substitute = is_focused
            .then(|| {
                let text = doc.text();
                let first = text.char_to_line(view_offset.anchor.min(text.len_chars()));
                let last = (first + inner.height as usize + 1).min(text.len_lines());
                let visible = text.line_to_char(first)..text.line_to_char(last);
                crate::commands::substitute_preview::inline(doc.id(), visible)
            })
            .flatten();
        let mut text_annotations = view.text_annotations(doc, Some(theme));
        let mut decorations = DecorationManager::default();
        if let Some((replacements, struck)) = &substitute {
            use crate::commands::substitute_preview as preview;
            text_annotations.add_inline_annotations(replacements, preview::added_highlight(theme));
            decorations.add_decoration(preview::StruckOut {
                ranges: struck.clone(),
                style: preview::struck_style(theme),
            });
        }

        // Plugins' line backgrounds, under the cursor line and the text.
        if let Some(backgrounds) = Self::plugin_line_backgrounds(doc, view, theme, inner) {
            decorations.add_decoration(backgrounds);
        }

        if is_focused && config.cursorline {
            decorations.add_decoration(Self::cursorline(doc, view, theme));
        }

        if is_focused && config.cursorcolumn {
            Self::highlight_cursorcolumn(doc, view, surface, theme, inner, &text_annotations);
        }

        // Set DAP highlights, if needed.
        if let Some(frame) = editor.current_stack_frame() {
            let dap_line = frame.line.saturating_sub(1);
            let style = theme.get("ui.highlight.frameline");
            let line_decoration = move |renderer: &mut TextRenderer, pos: LinePos| {
                if pos.doc_line != dap_line {
                    return;
                }
                renderer.set_style(Rect::new(inner.x, pos.visual_line, inner.width, 1), style);
            };

            decorations.add_decoration(line_decoration);
        }

        let syntax_highlighter =
            Self::doc_syntax_highlighter(doc, view_offset.anchor, inner.height, &loader);
        let mut overlays = Vec::new();

        overlays.push(Self::overlay_syntax_highlights(
            doc,
            view_offset.anchor,
            inner.height,
            &text_annotations,
        ));

        if doc
            .language_config()
            .and_then(|config| config.rainbow_brackets)
            .unwrap_or(config.rainbow_brackets)
        {
            if let Some(overlay) =
                Self::doc_rainbow_highlights(doc, view_offset.anchor, inner.height, theme, &loader)
            {
                overlays.push(overlay);
            }
        }

        if let Some(overlay) = Self::doc_document_link_highlights(doc, theme) {
            overlays.push(overlay);
        }

        Self::doc_diagnostics_highlights_into(doc, theme, &mut overlays);

        if !doc.plugin_decorations.is_empty() {
            let text = doc.text().slice(..);
            let first = text.char_to_line(view_offset.anchor.min(text.len_chars()));
            let last = (first + inner.height as usize * 2).min(text.len_lines() - 1);
            let end = text.line_to_char(last + 1).min(text.len_chars());
            let cursor_line = doc.plugin_decorations.wants_cursor_line().then(|| {
                let line = text.char_to_line(doc.selection(view.id).primary().cursor(text));
                text.line_to_char(line)..text.line_to_char((line + 1).min(text.len_lines()))
            });
            doc.plugin_decorations.overlays_into(
                theme,
                text.line_to_char(first)..end,
                cursor_line,
                &mut overlays,
            );
            if text_annotations.has_conceals() {
                overlays.push(
                    text_annotations.collect_conceal_highlights(text.line_to_char(first)..end),
                );
            }
        }

        if is_focused {
            if config.lsp.auto_document_highlight {
                if let Some(overlay) = Self::doc_document_highlights(doc, view, theme) {
                    overlays.push(overlay);
                }
            }
            if let Some(tabstops) = Self::tabstop_highlights(doc, theme) {
                overlays.push(tabstops);
            }
            overlays.push(Self::doc_selection_highlights(
                editor.mode(),
                doc,
                view,
                theme,
                &config.cursor_shape,
                self.terminal_focused,
            ));
            if let Some(overlay) = Self::highlight_focused_view_elements(view, doc, theme) {
                overlays.push(overlay);
            }
            if let Some(overlay) = crate::commands::substitute_preview::overlay(doc.id(), theme) {
                overlays.push(overlay);
            }
        }

        let gutter_overflow = view.gutter_offset(doc) == 0;
        if !gutter_overflow {
            Self::render_gutter(
                editor,
                doc,
                view,
                view.area,
                theme,
                is_focused & self.terminal_focused,
                &mut decorations,
            );
        }

        Self::render_rulers(editor, doc, view, inner, surface, theme);

        let primary_cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        if is_focused {
            decorations.add_decoration(text_decorations::Cursor {
                cache: &editor.cursor_cache,
                primary_cursor,
            });
        }
        let width = view.inner_width(doc);
        let config = doc.config.load();
        let enable_cursor_line = view
            .diagnostics_handler
            .show_cursorline_diagnostics(doc, view.id);
        let inline_diagnostic_config = config.inline_diagnostics.prepare(width, enable_cursor_line);
        decorations.add_decoration(InlineDiagnostics::new(
            doc,
            theme,
            primary_cursor,
            inline_diagnostic_config,
            config.end_of_line_diagnostics,
        ));
        render_document(
            surface,
            inner,
            doc,
            view_offset,
            &text_annotations,
            syntax_highlighter,
            overlays,
            theme,
            decorations,
        );

        // if we're not at the edge of the screen, draw a right border
        if viewport.right() != view.area.right() {
            let x = area.right();
            let border_style = theme.get("ui.window");
            for y in area.top()..area.bottom() {
                surface[(x, y)]
                    .set_symbol(tui::symbols::line::VERTICAL)
                    //.set_symbol(" ")
                    .set_style(border_style);
            }
        }

        if config.inline_diagnostics.disabled()
            && config.end_of_line_diagnostics == DiagnosticFilter::Disable
        {
            Self::render_diagnostics(doc, view, inner, surface, theme);
        }

        let statusline_area = view
            .area
            .clip_top(view.area.height.saturating_sub(1))
            .clip_bottom(1); // -1 from bottom to remove commandline

        let mut context =
            statusline::RenderContext::new(editor, doc, view, is_focused, &self.spinners);

        statusline::render(&mut context, statusline_area, surface);
    }

    pub fn render_rulers(
        editor: &Editor,
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        let editor_rulers = &editor.config().rulers;
        let ruler_theme = theme
            .try_get("ui.virtual.ruler")
            .unwrap_or_else(|| Style::default().bg(Color::Red));

        let rulers = doc
            .language_config()
            .and_then(|config| config.rulers.as_ref())
            .unwrap_or(editor_rulers);

        let view_offset = doc.view_offset(view.id);

        rulers
            .iter()
            // View might be horizontally scrolled, convert from absolute distance
            // from the 1st column to relative distance from left of viewport
            .filter_map(|ruler| ruler.checked_sub(1 + view_offset.horizontal_offset as u16))
            .filter(|ruler| ruler < &viewport.width)
            .map(|ruler| viewport.clip_left(ruler).with_width(1))
            .for_each(|area| surface.set_style(area, ruler_theme))
    }

    fn viewport_byte_range(
        text: helix_core::RopeSlice,
        row: usize,
        height: u16,
    ) -> std::ops::Range<usize> {
        // Calculate viewport byte ranges:
        // Saturating subs to make it inclusive zero indexing.
        let last_line = text.len_lines().saturating_sub(1);
        let last_visible_line = (row + height as usize).saturating_sub(1).min(last_line);
        let start = text.line_to_byte(row.min(last_line));
        let end = text.line_to_byte(last_visible_line + 1);

        start..end
    }

    /// Get the syntax highlighter for a document in a view represented by the first line
    /// and column (`offset`) and the last line. This is done instead of using a view
    /// directly to enable rendering syntax highlighted docs anywhere (eg. picker preview)
    pub fn doc_syntax_highlighter<'editor>(
        doc: &'editor Document,
        anchor: usize,
        height: u16,
        loader: &'editor syntax::Loader,
    ) -> Option<DocumentHighlighter<'editor>> {
        let syntax = doc.syntax()?;
        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));
        let range = Self::viewport_byte_range(text, row, height);
        let range = range.start as u32..range.end as u32;

        Some(DocumentHighlighter::new(syntax, text, loader, range))
    }

    pub fn overlay_syntax_highlights(
        doc: &Document,
        anchor: usize,
        height: u16,
        text_annotations: &TextAnnotations,
    ) -> OverlayHighlights {
        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));

        let mut range = Self::viewport_byte_range(text, row, height);
        range = text.byte_to_char(range.start)..text.byte_to_char(range.end);

        text_annotations.collect_overlay_highlights(range)
    }

    pub fn doc_rainbow_highlights(
        doc: &Document,
        anchor: usize,
        height: u16,
        theme: &Theme,
        loader: &syntax::Loader,
    ) -> Option<OverlayHighlights> {
        let syntax = doc.syntax()?;
        let text = doc.text().slice(..);
        let row = text.char_to_line(anchor.min(text.len_chars()));
        let visible_range = Self::viewport_byte_range(text, row, height);
        let start = syntax::child_for_byte_range(
            &syntax.tree().root_node(),
            visible_range.start as u32..visible_range.end as u32,
        )
        .map_or(visible_range.start as u32, |node| node.start_byte());
        let range = start..visible_range.end as u32;

        Some(syntax.rainbow_highlights(text, theme.rainbow_length(), loader, range))
    }

    /// Get highlight spans for document diagnostics
    pub fn doc_diagnostics_highlights_into(
        doc: &Document,
        theme: &Theme,
        overlay_highlights: &mut Vec<OverlayHighlights>,
    ) {
        // Skip redundant work if no diagnostics.
        if doc.diagnostics().is_empty() {
            return;
        }

        use helix_core::diagnostic::{DiagnosticTag, Range, Severity};
        let get_scope_of = |scope| {
            theme
                .find_highlight_exact(scope)
                // get one of the themes below as fallback values
                .or_else(|| theme.find_highlight_exact("diagnostic"))
                .or_else(|| theme.find_highlight_exact("ui.cursor"))
                .or_else(|| theme.find_highlight_exact("ui.selection"))
                .expect(
                    "at least one of the following scopes must be defined in the theme: `diagnostic`, `ui.cursor`, or `ui.selection`",
                )
        };

        // Diagnostic tags
        let unnecessary = theme.find_highlight_exact("diagnostic.unnecessary");
        let deprecated = theme.find_highlight_exact("diagnostic.deprecated");

        let mut default_vec = Vec::new();
        let mut info_vec = Vec::new();
        let mut hint_vec = Vec::new();
        let mut warning_vec = Vec::new();
        let mut error_vec = Vec::new();
        let mut unnecessary_vec = Vec::new();
        let mut deprecated_vec = Vec::new();

        let push_diagnostic = |vec: &mut Vec<ops::Range<usize>>, range: Range| {
            // If any diagnostic overlaps ranges with the prior diagnostic,
            // merge the two together. Otherwise push a new span.
            match vec.last_mut() {
                Some(existing_range) if range.start <= existing_range.end => {
                    // This branch merges overlapping diagnostics, assuming that the current
                    // diagnostic starts on range.start or later. If this assertion fails,
                    // we will discard some part of `diagnostic`. This implies that
                    // `doc.diagnostics()` is not sorted by `diagnostic.range`.
                    debug_assert!(existing_range.start <= range.start);
                    existing_range.end = range.end.max(existing_range.end)
                }
                _ => vec.push(range.start..range.end),
            }
        };

        for diagnostic in doc.diagnostics() {
            // Separate diagnostics into different Vecs by severity.
            let vec = match diagnostic.severity {
                Some(Severity::Info) => &mut info_vec,
                Some(Severity::Hint) => &mut hint_vec,
                Some(Severity::Warning) => &mut warning_vec,
                Some(Severity::Error) => &mut error_vec,
                _ => &mut default_vec,
            };

            // If the diagnostic has tags and a non-warning/error severity, skip rendering
            // the diagnostic as info/hint/default and only render it as unnecessary/deprecated
            // instead. For warning/error diagnostics, render both the severity highlight and
            // the tag highlight.
            if diagnostic.tags.is_empty()
                || matches!(
                    diagnostic.severity,
                    Some(Severity::Warning | Severity::Error)
                )
            {
                push_diagnostic(vec, diagnostic.range);
            }

            for tag in &diagnostic.tags {
                match tag {
                    DiagnosticTag::Unnecessary => {
                        if unnecessary.is_some() {
                            push_diagnostic(&mut unnecessary_vec, diagnostic.range)
                        }
                    }
                    DiagnosticTag::Deprecated => {
                        if deprecated.is_some() {
                            push_diagnostic(&mut deprecated_vec, diagnostic.range)
                        }
                    }
                }
            }
        }

        overlay_highlights.push(OverlayHighlights::Homogeneous {
            highlight: get_scope_of("diagnostic"),
            ranges: default_vec,
        });
        if let Some(highlight) = unnecessary {
            overlay_highlights.push(OverlayHighlights::Homogeneous {
                highlight,
                ranges: unnecessary_vec,
            });
        }
        if let Some(highlight) = deprecated {
            overlay_highlights.push(OverlayHighlights::Homogeneous {
                highlight,
                ranges: deprecated_vec,
            });
        }
        overlay_highlights.extend([
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.info"),
                ranges: info_vec,
            },
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.hint"),
                ranges: hint_vec,
            },
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.warning"),
                ranges: warning_vec,
            },
            OverlayHighlights::Homogeneous {
                highlight: get_scope_of("diagnostic.error"),
                ranges: error_vec,
            },
        ]);
    }

    pub fn doc_document_highlights(
        doc: &Document,
        view: &View,
        theme: &Theme,
    ) -> Option<OverlayHighlights> {
        let ranges = doc.document_highlights(view.id)?;
        if ranges.is_empty() {
            return None;
        }

        let highlight = theme
            .find_highlight_exact("ui.highlight")
            .or_else(|| theme.find_highlight_exact("ui.selection"))
            .or_else(|| theme.find_highlight_exact("ui.cursor"))?;

        Some(OverlayHighlights::Homogeneous {
            highlight,
            ranges: ranges.to_vec(),
        })
    }

    pub fn doc_document_link_highlights(
        doc: &Document,
        theme: &Theme,
    ) -> Option<OverlayHighlights> {
        let highlight = theme
            .find_highlight_exact("markup.link.url")
            .or_else(|| theme.find_highlight_exact("markup.link"))?;

        if doc.document_links.is_empty() {
            return None;
        }

        let mut ranges: Vec<ops::Range<usize>> = Vec::new();
        for link in &doc.document_links {
            if link.start >= link.end {
                continue;
            }

            match ranges.last_mut() {
                Some(existing_range) if link.start <= existing_range.end => {
                    existing_range.end = existing_range.end.max(link.end);
                }
                _ => ranges.push(link.start..link.end),
            }
        }

        if ranges.is_empty() {
            return None;
        }

        Some(OverlayHighlights::Homogeneous { highlight, ranges })
    }

    /// Get highlight spans for selections in a document view.
    pub fn doc_selection_highlights(
        mode: Mode,
        doc: &Document,
        view: &View,
        theme: &Theme,
        cursor_shape_config: &CursorShapeConfig,
        is_terminal_focused: bool,
    ) -> OverlayHighlights {
        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);
        let primary_idx = selection.primary_index();

        let cursorkind = cursor_shape_config.from_mode(mode);
        let cursor_is_block = cursorkind == CursorKind::Block;

        let selection_scope = theme
            .find_highlight_exact("ui.selection")
            .expect("could not find `ui.selection` scope in the theme!");
        let primary_selection_scope = theme
            .find_highlight_exact("ui.selection.primary")
            .unwrap_or(selection_scope);

        let base_cursor_scope = theme
            .find_highlight_exact("ui.cursor")
            .unwrap_or(selection_scope);
        let base_primary_cursor_scope = theme
            .find_highlight("ui.cursor.primary")
            .unwrap_or(base_cursor_scope);

        let cursor_scope = match mode {
            Mode::Insert => theme.find_highlight_exact("ui.cursor.insert"),
            Mode::Select => theme.find_highlight_exact("ui.cursor.select"),
            Mode::Normal => theme.find_highlight_exact("ui.cursor.normal"),
        }
        .unwrap_or(base_cursor_scope);

        let primary_cursor_scope = match mode {
            Mode::Insert => theme.find_highlight_exact("ui.cursor.primary.insert"),
            Mode::Select => theme.find_highlight_exact("ui.cursor.primary.select"),
            Mode::Normal => theme.find_highlight_exact("ui.cursor.primary.normal"),
        }
        .unwrap_or(base_primary_cursor_scope);

        let mut spans = Vec::new();
        for (i, range) in selection.iter().enumerate() {
            let selection_is_primary = i == primary_idx;
            let (cursor_scope, selection_scope) = if selection_is_primary {
                (primary_cursor_scope, primary_selection_scope)
            } else {
                (cursor_scope, selection_scope)
            };

            // Special-case: cursor at end of the rope.
            if range.head == range.anchor && range.head == text.len_chars() {
                if !selection_is_primary || (cursor_is_block && is_terminal_focused) {
                    // Bar and underline cursors are drawn by the terminal
                    // BUG: If the editor area loses focus while having a bar or
                    // underline cursor (eg. when a regex prompt has focus) then
                    // the primary cursor will be invisible. This doesn't happen
                    // with block cursors since we manually draw *all* cursors.
                    spans.push((cursor_scope, range.head..range.head + 1));
                }
                continue;
            }

            let range = range.min_width_1(text);
            if range.head > range.anchor {
                // Standard case.
                let cursor_start = prev_grapheme_boundary(text, range.head);
                // non block cursors look like they exclude the cursor
                let selection_end =
                    if selection_is_primary && !cursor_is_block && mode != Mode::Insert {
                        range.head
                    } else {
                        cursor_start
                    };
                spans.push((selection_scope, range.anchor..selection_end));
                // add block cursors
                // skip primary cursor if terminal is unfocused - terminal cursor is used in that case
                if !selection_is_primary || (cursor_is_block && is_terminal_focused) {
                    spans.push((cursor_scope, cursor_start..range.head));
                }
            } else {
                // Reverse case.
                let cursor_end = next_grapheme_boundary(text, range.head);
                // add block cursors
                // skip primary cursor if terminal is unfocused - terminal cursor is used in that case
                if !selection_is_primary || (cursor_is_block && is_terminal_focused) {
                    spans.push((cursor_scope, range.head..cursor_end));
                }
                // non block cursors look like they exclude the cursor
                let selection_start = if selection_is_primary
                    && !cursor_is_block
                    && !(mode == Mode::Insert && cursor_end == range.anchor)
                {
                    range.head
                } else {
                    cursor_end
                };
                spans.push((selection_scope, selection_start..range.anchor));
            }
        }

        OverlayHighlights::Heterogenous { highlights: spans }
    }

    /// Render brace match, etc (meant for the focused view only)
    pub fn highlight_focused_view_elements(
        view: &View,
        doc: &Document,
        theme: &Theme,
    ) -> Option<OverlayHighlights> {
        // Highlight matching braces
        let syntax = doc.syntax()?;
        let highlight = theme.find_highlight_exact("ui.cursor.match")?;
        let text = doc.text().slice(..);
        let pos = doc.selection(view.id).primary().cursor(text);
        let pos = helix_core::match_brackets::find_matching_bracket(syntax, text, pos)?;
        Some(OverlayHighlights::single(highlight, pos..pos + 1))
    }

    pub fn tabstop_highlights(doc: &Document, theme: &Theme) -> Option<OverlayHighlights> {
        let snippet = doc.active_snippet.as_ref()?;
        let highlight = theme.find_highlight_exact("tabstop")?;
        let mut ranges = Vec::new();
        for tabstop in snippet.tabstops() {
            ranges.extend(tabstop.ranges.iter().map(|range| range.start..range.end));
        }
        Some(OverlayHighlights::Homogeneous { highlight, ranges })
    }

    /// Where to show the startup screen: only while the editor is just one
    /// window with an empty, unchanged scratch buffer (as after starting
    /// without files). It goes away as soon as anything is typed or opened.
    fn splash_area(editor: &Editor) -> Option<Rect> {
        if editor.documents.len() != 1 || editor.tree.views().count() != 1 {
            return None;
        }
        let view = editor.tree.get(editor.tree.focus);
        let doc = editor.document(view.doc)?;
        let text = doc.text();
        let empty = text.len_chars() == 0 || (text.len_chars() == 1 && text.char(0) == '\n');
        (doc.path().is_none() && empty && !doc.is_modified()).then(|| view.inner_area(doc))
    }

    fn render_splash(editor: &Editor, area: Rect, surface: &mut Surface) {
        let theme = &editor.theme;
        let name = theme.get("ui.text.focus").add_modifier(Modifier::BOLD);
        let text = theme.get("ui.text");
        let quote = text.add_modifier(Modifier::ITALIC);
        let hint = theme
            .try_get("ui.text.inactive")
            .unwrap_or_else(|| theme.get("comment"));

        // Wrap the quote to a comfortable width.
        let width = (area.width as usize).saturating_sub(4).clamp(10, 64);
        let mut quote_lines: Vec<String> = Vec::new();
        for word in format!("“{}”", crate::QUOTE).split_whitespace() {
            match quote_lines.last_mut() {
                Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                    line.push(' ');
                    line.push_str(word);
                }
                _ => quote_lines.push(word.to_string()),
            }
        }

        let title = crate::NAME
            .to_uppercase()
            .chars()
            .map(String::from)
            .collect::<Vec<_>>()
            .join(" ");
        let mut lines: Vec<(String, Style)> = vec![
            (title, name),
            ("a modal editor, forked from Helix".into(), hint),
            (String::new(), text),
        ];
        lines.extend(quote_lines.into_iter().map(|line| (line, quote)));
        lines.push((String::new(), text));
        for (keys, what) in [
            ("space f", "open a file"),
            ("space x", "run a command"),
            (":tutor ", "learn the basics"),
            (":q     ", "quit"),
        ] {
            lines.push((format!("{keys}   {what:<16}"), hint));
        }

        if (area.height as usize) < lines.len() || area.width < 20 {
            return;
        }
        let top = area.y + (area.height - lines.len() as u16) / 2;
        for (i, (line, style)) in lines.iter().enumerate() {
            let len = line.chars().count().min(area.width as usize) as u16;
            let x = area.x + (area.width - len) / 2;
            surface.set_stringn(x, top + i as u16, line, area.width as usize, *style);
        }
    }

    /// Render bufferline at the top
    pub fn render_bufferline(editor: &Editor, viewport: Rect, surface: &mut Surface) {
        let scratch = PathBuf::from(SCRATCH_BUFFER_NAME); // default filename to use for scratch buffer
        surface.clear_with(
            viewport,
            editor
                .theme
                .try_get("ui.bufferline.background")
                .unwrap_or_else(|| editor.theme.get("ui.statusline")),
        );

        let bufferline_active = editor
            .theme
            .try_get("ui.bufferline.active")
            .unwrap_or_else(|| editor.theme.get("ui.statusline.active"));

        let bufferline_inactive = editor
            .theme
            .try_get("ui.bufferline")
            .unwrap_or_else(|| editor.theme.get("ui.statusline.inactive"));

        let mut x = viewport.x;
        let current_doc = view!(editor).doc;

        for doc in editor.documents() {
            let fname = match (doc.path(), &doc.name) {
                (None, Some(name)) => name,
                (path, _) => path
                    .unwrap_or(&scratch)
                    .file_name()
                    .unwrap_or_default()
                    .to_str()
                    .unwrap_or_default(),
            };

            let style = if current_doc == doc.id() {
                bufferline_active
            } else {
                bufferline_inactive
            };

            let text = format!(" {}{} ", fname, if doc.is_modified() { "[+]" } else { "" });
            let used_width = viewport.x.saturating_sub(x);
            let rem_width = surface.area.width.saturating_sub(used_width);

            x = surface
                .set_stringn(x, viewport.y, &text, rem_width as usize, style)
                .0;

            if x >= surface.area.right() {
                break;
            }
        }
    }

    pub fn render_gutter<'d>(
        editor: &'d Editor,
        doc: &'d Document,
        view: &View,
        viewport: Rect,
        theme: &Theme,
        is_focused: bool,
        decoration_manager: &mut DecorationManager<'d>,
    ) {
        let text = doc.text().slice(..);
        let cursors: Rc<[_]> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let mut offset = 0;

        let gutter_style = theme.get("ui.gutter");
        let gutter_selected_style = theme.get("ui.gutter.selected");
        let gutter_style_virtual = theme.get("ui.gutter.virtual");
        let gutter_selected_style_virtual = theme.get("ui.gutter.selected.virtual");

        for gutter_type in view.gutters() {
            let mut gutter = gutter_type.style(editor, doc, view, theme, is_focused);
            let width = gutter_type.width(view, doc);
            // avoid lots of small allocations by reusing a text buffer for each line
            let mut text = String::with_capacity(width);
            let cursors = cursors.clone();
            let gutter_decoration = move |renderer: &mut TextRenderer, pos: LinePos| {
                // TODO handle softwrap in gutters
                let selected = cursors.contains(&pos.doc_line);
                let x = viewport.x + offset;
                let y = pos.visual_line;

                let gutter_style = match (selected, pos.first_visual_line) {
                    (false, true) => gutter_style,
                    (true, true) => gutter_selected_style,
                    (false, false) => gutter_style_virtual,
                    (true, false) => gutter_selected_style_virtual,
                };

                if let Some(style) =
                    gutter(pos.doc_line, selected, pos.first_visual_line, &mut text)
                {
                    renderer.set_stringn(x, y, &text, width, gutter_style.patch(style));
                } else {
                    renderer.set_style(
                        Rect {
                            x,
                            y,
                            width: width as u16,
                            height: 1,
                        },
                        gutter_style,
                    );
                }
                text.clear();
            };
            decoration_manager.add_decoration(gutter_decoration);

            offset += width as u16;
        }
    }

    pub fn render_diagnostics(
        doc: &Document,
        view: &View,
        viewport: Rect,
        surface: &mut Surface,
        theme: &Theme,
    ) {
        use helix_core::diagnostic::Severity;
        use tui::{
            layout::Alignment,
            text::Text,
            widgets::{Paragraph, Widget, Wrap},
        };

        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));

        let diagnostics = doc.diagnostics().iter().filter(|diagnostic| {
            diagnostic.range.start <= cursor && diagnostic.range.end >= cursor
        });

        let warning = theme.get("warning");
        let error = theme.get("error");
        let info = theme.get("info");
        let hint = theme.get("hint");

        let mut lines = Vec::new();
        let background_style = theme.get("ui.background");
        for diagnostic in diagnostics {
            let style = Style::reset()
                .patch(background_style)
                .patch(match diagnostic.severity {
                    Some(Severity::Error) => error,
                    Some(Severity::Warning) | None => warning,
                    Some(Severity::Info) => info,
                    Some(Severity::Hint) => hint,
                });
            let text = Text::styled(&diagnostic.message, style);
            lines.extend(text.lines);
            let code = diagnostic.code.as_ref().map(|x| match x {
                NumberOrString::Number(n) => format!("({n})"),
                NumberOrString::String(s) => format!("({s})"),
            });
            if let Some(code) = code {
                let span = Span::styled(code, style);
                lines.push(span.into());
            }
        }

        let text = Text::from(lines);
        let paragraph = Paragraph::new(&text)
            .alignment(Alignment::Right)
            .wrap(Wrap { trim: true });
        let width = 100.min(viewport.width);
        let height = 15.min(viewport.height);
        paragraph.render(
            Rect::new(viewport.right() - width, viewport.y + 1, width, height),
            surface,
        );
    }

    /// Apply the highlighting on the lines where a cursor is active
    /// Plugins' whole-line backgrounds for the visible lines, filling the
    /// text area of the view (not the gutter).
    fn plugin_line_backgrounds(
        doc: &Document,
        view: &View,
        theme: &Theme,
        inner: Rect,
    ) -> Option<impl Decoration> {
        if doc.plugin_decorations.is_empty() {
            return None;
        }
        let text = doc.text().slice(..);
        let anchor = doc.view_offset(view.id).anchor.min(text.len_chars());
        let first = text.char_to_line(anchor);
        // Wrapped lines take more rows, never fewer: this covers the view.
        let lines = first..first + inner.height as usize + 1;
        let cursor_line = doc
            .plugin_decorations
            .wants_cursor_line()
            .then(|| text.char_to_line(doc.selection(view.id).primary().cursor(text)));
        let styles = doc
            .plugin_decorations
            .line_backgrounds(text, theme, lines, cursor_line);
        (!styles.is_empty()).then_some(move |renderer: &mut TextRenderer, pos: LinePos| {
            if let Some(style) = styles.get(&pos.doc_line) {
                renderer.set_style(Rect::new(inner.x, pos.visual_line, inner.width, 1), *style);
            }
        })
    }

    pub fn cursorline(doc: &Document, view: &View, theme: &Theme) -> impl Decoration {
        let text = doc.text().slice(..);
        // TODO only highlight the visual line that contains the cursor instead of the full visual line
        let primary_line = doc.selection(view.id).primary().cursor_line(text);

        // The secondary_lines do contain the primary_line, it doesn't matter
        // as the else-if clause in the loop later won't test for the
        // secondary_lines if primary_line == line.
        // It's used inside a loop so the collect isn't needless:
        // https://github.com/rust-lang/rust-clippy/issues/6164
        #[allow(clippy::needless_collect)]
        let secondary_lines: Vec<_> = doc
            .selection(view.id)
            .iter()
            .map(|range| range.cursor_line(text))
            .collect();

        let primary_style = theme.get("ui.cursorline.primary");
        let secondary_style = theme.get("ui.cursorline.secondary");
        let viewport = view.area;

        move |renderer: &mut TextRenderer, pos: LinePos| {
            let area = Rect::new(viewport.x, pos.visual_line, viewport.width, 1);
            if primary_line == pos.doc_line {
                renderer.set_style(area, primary_style);
            } else if secondary_lines.binary_search(&pos.doc_line).is_ok() {
                renderer.set_style(area, secondary_style);
            }
        }
    }

    /// Apply the highlighting on the columns where a cursor is active
    pub fn highlight_cursorcolumn(
        doc: &Document,
        view: &View,
        surface: &mut Surface,
        theme: &Theme,
        viewport: Rect,
        text_annotations: &TextAnnotations,
    ) {
        let text = doc.text().slice(..);

        // Manual fallback behaviour:
        // ui.cursorcolumn.{p/s} -> ui.cursorcolumn -> ui.cursorline.{p/s}
        let primary_style = theme
            .try_get_exact("ui.cursorcolumn.primary")
            .or_else(|| theme.try_get_exact("ui.cursorcolumn"))
            .unwrap_or_else(|| theme.get("ui.cursorline.primary"));
        let secondary_style = theme
            .try_get_exact("ui.cursorcolumn.secondary")
            .or_else(|| theme.try_get_exact("ui.cursorcolumn"))
            .unwrap_or_else(|| theme.get("ui.cursorline.secondary"));

        let inner_area = view.inner_area(doc);

        let selection = doc.selection(view.id);
        let view_offset = doc.view_offset(view.id);
        let primary = selection.primary();
        let text_format = doc.text_format(viewport.width, None);
        for range in selection.iter() {
            let is_primary = primary == *range;
            let cursor = range.cursor(text);

            let Position { col, .. } =
                visual_offset_from_block(text, cursor, cursor, &text_format, text_annotations).0;

            // if the cursor is horizontally in the view
            if col >= view_offset.horizontal_offset
                && inner_area.width > (col - view_offset.horizontal_offset) as u16
            {
                let area = Rect::new(
                    inner_area.x + (col - view_offset.horizontal_offset) as u16,
                    view.area.y,
                    1,
                    view.area.height,
                );
                if is_primary {
                    surface.set_style(area, primary_style)
                } else {
                    surface.set_style(area, secondary_style)
                }
            }
        }
    }

    /// Handle events by looking them up in `self.keymaps`. Returns None
    /// if event was handled (a command was executed or a subkeymap was
    /// activated). Only KeymapResult::{NotFound, Cancelled} is returned
    /// otherwise.
    fn handle_keymap_event(
        &mut self,
        mode: Mode,
        cxt: &mut commands::Context,
        event: KeyEvent,
    ) -> Option<KeymapResult> {
        let mut last_mode = mode;
        self.pseudo_pending.extend(self.keymaps.pending());
        // The keys typed so far, for plugin bindings.
        let mut sequence = self.keymaps.pending().to_vec();
        sequence.push(event);
        let (owner, document, language) = {
            let doc = doc!(cxt.editor);
            // Languages only matter to plugin bindings.
            let language = crate::keymap::plugin_layers::has_bindings()
                .then(|| doc.language_name().map(str::to_string))
                .flatten();
            (doc.plugin_owner.clone(), doc.id(), language)
        };
        let scope = crate::keymap::plugin_layers::KeyScope {
            owner: owner.as_deref(),
            document: Some(document),
            language: language.as_deref(),
        };
        let key_result = self.keymaps.get_scoped(mode, event, &scope);
        cxt.editor.autoinfo = self.keymaps.sticky().map(|node| node.infobox());

        // Keys bound to a plugin callback: ask the plugin. If it passes them
        // through, they do what they would without the binding.
        let plugin_binding = match &key_result {
            KeymapResult::Matched(command) => crate::keymap::plugin_layers::binding_of(command),
            _ => None,
        };
        let mut passed_through = None;
        if let Some((plugin, name)) = &plugin_binding {
            let keys: Vec<String> = sequence.iter().map(|key| key.to_string()).collect();
            let count = cxt.count.map(|count| count.get() as u32);
            let handled =
                crate::plugins::key(cxt.editor, cxt.jobs, plugin, name, &keys.join(" "), count);
            if !handled {
                match self
                    .keymaps
                    .without_binding(mode, &sequence, &scope, (plugin, name))
                {
                    Some(trie @ (KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_))) => {
                        passed_through = Some(trie)
                    }
                    _ if sequence.len() == 1 => return Some(KeymapResult::NotFound),
                    _ => return Some(KeymapResult::Cancelled(sequence)),
                }
            }
            // The plugin's actions may have changed the mode.
            let current_mode = cxt.editor.mode();
            if current_mode != last_mode {
                helix_event::dispatch(OnModeSwitch {
                    old_mode: last_mode,
                    new_mode: current_mode,
                    cx: cxt,
                });
                last_mode = current_mode;
            }
        }

        let mut execute_command = |command: &commands::MappableCommand| {
            command.execute(cxt);
            helix_event::dispatch(PostCommand { command, cx: cxt });

            let current_mode = cxt.editor.mode();
            if current_mode != last_mode {
                helix_event::dispatch(OnModeSwitch {
                    old_mode: last_mode,
                    new_mode: current_mode,
                    cx: cxt,
                });

                // HAXX: if we just entered insert mode from normal, clear key buf
                // and record the command that got us into this mode.
                if current_mode == Mode::Insert {
                    // how we entered insert mode is important, and we should track that so
                    // we can repeat the side effect.
                    self.last_insert.0 = command.clone();
                    self.last_insert.1.clear();
                }
            }

            last_mode = current_mode;
        };

        match &key_result {
            KeymapResult::Matched(command) => match &plugin_binding {
                // A plugin binding the plugin passed through: what the keys
                // do without it.
                Some(_) => match passed_through {
                    Some(KeyTrie::MappableCommand(command)) => execute_command(&command),
                    Some(KeyTrie::Sequence(commands)) => {
                        for command in &commands {
                            execute_command(command);
                        }
                    }
                    _ => {}
                },
                None => execute_command(command),
            },
            KeymapResult::Pending(node) => cxt.editor.autoinfo = Some(node.infobox()),
            KeymapResult::MatchedSequence(commands) => {
                for command in commands {
                    execute_command(command);
                }
            }
            KeymapResult::NotFound | KeymapResult::Cancelled(_) => return Some(key_result),
        }
        None
    }

    fn insert_mode(&mut self, cx: &mut commands::Context, event: KeyEvent) {
        if let Some(keyresult) = self.handle_keymap_event(Mode::Insert, cx, event) {
            match keyresult {
                KeymapResult::NotFound => {
                    if !self.on_next_key(OnKeyCallbackKind::Fallback, cx, event) {
                        if let Some(ch) = event.char() {
                            commands::insert::insert_char(cx, ch)
                        }
                    }
                }
                KeymapResult::Cancelled(pending) => {
                    for ev in pending {
                        match ev.char() {
                            Some(ch) => commands::insert::insert_char(cx, ch),
                            None => {
                                if let KeymapResult::Matched(command) =
                                    self.keymaps.get(Mode::Insert, ev)
                                {
                                    command.execute(cx);
                                }
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    fn command_mode(&mut self, mode: Mode, cxt: &mut commands::Context, event: KeyEvent) {
        match (event, cxt.editor.count) {
            // If the count is already started and the input is a number, always continue the count.
            (key!(i @ '0'..='9'), Some(count)) => {
                let i = i.to_digit(10).unwrap() as usize;
                let count = count.get() * 10 + i;
                if count > 100_000_000 {
                    return;
                }
                cxt.editor.count = NonZeroUsize::new(count);
            }
            // A non-zero digit will start the count if that number isn't used by a keymap.
            (key!(i @ '1'..='9'), None) if !self.keymaps.contains_key(mode, event) => {
                let i = i.to_digit(10).unwrap() as usize;
                cxt.editor.count = NonZeroUsize::new(i);
            }
            // special handling for repeat operator
            (key!('.'), _) if self.keymaps.pending().is_empty() => {
                for _ in 0..cxt.editor.count.map_or(1, NonZeroUsize::into) {
                    // first execute whatever put us into insert mode
                    self.last_insert.0.execute(cxt);
                    let mut last_savepoint = None;
                    let mut last_request_savepoint = None;
                    // then replay the inputs
                    for key in self.last_insert.1.clone() {
                        match key {
                            InsertEvent::Key(key) => self.insert_mode(cxt, key),
                            InsertEvent::CompletionApply {
                                trigger_offset,
                                changes,
                            } => {
                                let (view, doc) = current!(cxt.editor);

                                if let Some(last_savepoint) = last_savepoint.as_deref() {
                                    doc.restore(view, last_savepoint, true);
                                }

                                let text = doc.text().slice(..);
                                let cursor = doc.selection(view.id).primary().cursor(text);

                                let shift_position = |pos: usize| -> usize {
                                    (pos + cursor).saturating_sub(trigger_offset)
                                };

                                let tx = Transaction::change(
                                    doc.text(),
                                    changes.iter().cloned().map(|(start, end, t)| {
                                        (shift_position(start), shift_position(end), t)
                                    }),
                                );
                                doc.apply(&tx, view.id);
                            }
                            InsertEvent::TriggerCompletion => {
                                last_savepoint = take(&mut last_request_savepoint);
                            }
                            InsertEvent::RequestCompletion => {
                                let (view, doc) = current!(cxt.editor);
                                last_request_savepoint = Some(doc.savepoint(view));
                            }
                        }
                    }
                }
                cxt.editor.count = None;
            }
            _ => {
                // set the count
                cxt.count = cxt.editor.count;
                // TODO: edge case: 0j -> reset to 1
                // if this fails, count was Some(0)
                // debug_assert!(cxt.count != 0);

                // set the register
                cxt.register = cxt.editor.selected_register.take();

                let res = self.handle_keymap_event(mode, cxt, event);
                if matches!(&res, Some(KeymapResult::NotFound)) {
                    self.on_next_key(OnKeyCallbackKind::Fallback, cxt, event);
                }
                if self.keymaps.pending().is_empty() {
                    cxt.editor.count = None
                } else {
                    cxt.editor.selected_register = cxt.register.take();
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_completion(
        &mut self,
        editor: &mut Editor,
        items: Vec<CompletionItem>,
        trigger_offset: usize,
        size: Rect,
    ) -> Option<Rect> {
        let mut completion = Completion::new(editor, items, trigger_offset);

        if completion.is_empty() {
            // skip if we got no completion results
            return None;
        }

        let area = completion.area(size, editor);
        editor.last_completion = Some(CompleteAction::Triggered);
        self.last_insert.1.push(InsertEvent::TriggerCompletion);

        {
            use crate::ui::completion::menu_events::{self, Close, MenuEvent};
            if self.completion.is_some() {
                crate::plugins::completion_menu_event(editor, MenuEvent::Closed(Close::Replaced));
            }
            let menu = completion.menu_state(editor);
            crate::plugins::completion_menu_event(editor, MenuEvent::Opened(menu.count));
            menu_events::set(Some(menu));
        }
        // TODO : propagate required size on resize to completion too
        self.completion = Some(completion);
        Some(area)
    }

    /// The completion menu changed (selection, filter, items): update what
    /// plugins see, and tell them if the items did.
    pub fn sync_menu(&mut self, editor: &mut Editor) {
        use crate::ui::completion::menu_events::{self, MenuEvent};
        let Some(completion) = &self.completion else {
            return;
        };
        let menu = completion.menu_state(editor);
        let before = menu_events::snapshot().map(|menu| menu.count);
        if before != Some(menu.count) {
            crate::plugins::completion_menu_event(editor, MenuEvent::Updated(menu.count));
        }
        menu_events::set(Some(menu));
    }

    /// A key of `completion-keys` while the menu is open.
    fn completion_key(
        &mut self,
        action: helix_view::editor::CompletionKeyAction,
        cx: &mut commands::Context,
    ) {
        use crate::ui::completion::menu_events::MenuCommand;
        use helix_view::editor::CompletionKeyAction as A;
        let mut compositor_cx = crate::compositor::Context {
            editor: cx.editor,
            jobs: cx.jobs,
            scroll: None,
        };
        let command = match action {
            A::Confirm => MenuCommand::Confirm,
            A::Abort | A::NormalMode => MenuCommand::Abort,
            A::Close => MenuCommand::Close,
            A::Next => MenuCommand::Next,
            A::Previous => MenuCommand::Previous,
            A::PageUp | A::PageDown => {
                if let Some(completion) = &mut self.completion {
                    let key = if action == A::PageUp {
                        key!(PageUp)
                    } else {
                        key!(PageDown)
                    };
                    completion.handle_event(&Event::Key(key), &mut compositor_cx);
                }
                self.sync_menu(cx.editor);
                return;
            }
        };
        self.completion_command(command, &mut compositor_cx);
        if action == A::NormalMode {
            let old_mode = cx.editor.mode();
            commands::MappableCommand::normal_mode.execute(cx);
            let new_mode = cx.editor.mode();
            if new_mode != old_mode {
                helix_event::dispatch(OnModeSwitch {
                    old_mode,
                    new_mode,
                    cx,
                });
            }
        }
    }

    /// A plugin drives the completion menu (API 0.5), through the menu's
    /// own code: previews, undo and language server sync work as with the
    /// keys.
    pub fn completion_command(
        &mut self,
        command: crate::ui::completion::menu_events::MenuCommand,
        cx: &mut crate::compositor::Context,
    ) {
        use crate::ui::completion::menu_events::{set_close_reason, Close, MenuCommand, MenuEvent};
        let Some(completion) = &mut self.completion else {
            return;
        };
        let accept = |view: &mut Self, cx: &mut crate::compositor::Context, close: Close| {
            let Some(completion) = &mut view.completion else {
                return;
            };
            let label = completion.selected_label();
            if let EventResult::Consumed(Some(_)) =
                completion.handle_event(&Event::Key(key!(Enter)), cx)
            {
                if let (Close::Confirmed, Some(label)) = (close, label) {
                    crate::plugins::completion_menu_event(cx.editor, MenuEvent::Confirmed(label));
                }
                set_close_reason(close);
                if let Some(cb) = view.clear_completion(cx.editor) {
                    view.on_next_key = Some((cb, OnKeyCallbackKind::Fallback));
                }
            }
        };
        match command {
            MenuCommand::Select(index) => {
                completion.select(index.map(|i| i as usize), cx.editor);
                self.sync_menu(cx.editor);
            }
            MenuCommand::Next | MenuCommand::Previous => {
                let key = if command == MenuCommand::Next {
                    crate::ctrl!('n')
                } else {
                    crate::ctrl!('p')
                };
                completion.handle_event(&Event::Key(key), cx);
                self.sync_menu(cx.editor);
            }
            MenuCommand::Confirm => {
                if completion.selected_label().is_none() {
                    completion.select(Some(0), cx.editor);
                }
                accept(self, cx, Close::Confirmed);
            }
            MenuCommand::Abort => {
                set_close_reason(Close::Aborted);
                self.clear_completion(cx.editor);
            }
            // A previewed item is accepted (as typing a character would);
            // without one the text stays as it is.
            MenuCommand::Close => {
                if completion.selected_label().is_some() {
                    accept(self, cx, Close::Dismissed);
                } else {
                    set_close_reason(Close::Dismissed);
                    self.clear_completion(cx.editor);
                }
            }
        }
    }

    pub fn clear_completion(&mut self, editor: &mut Editor) -> Option<OnKeyCallback> {
        {
            use crate::ui::completion::menu_events::{self, MenuEvent};
            let reason = menu_events::take_close_reason();
            if self.completion.is_some() {
                crate::plugins::completion_menu_event(editor, MenuEvent::Closed(reason));
                menu_events::set(None);
            }
        }
        self.completion = None;
        let mut on_next_key: Option<OnKeyCallback> = None;
        editor.handlers.completions.request_controller.restart();
        editor.handlers.completions.active_completions.clear();
        if let Some(last_completion) = editor.last_completion.take() {
            match last_completion {
                CompleteAction::Triggered => (),
                CompleteAction::Applied {
                    trigger_offset,
                    changes,
                    placeholder,
                } => {
                    self.last_insert.1.push(InsertEvent::CompletionApply {
                        trigger_offset,
                        changes,
                    });
                    on_next_key = placeholder.then_some(Box::new(|cx, key| {
                        if let Some(c) = key.char() {
                            let (view, doc) = current!(cx.editor);
                            if let Some(snippet) = &doc.active_snippet {
                                doc.apply(&snippet.delete_placeholder(doc.text()), view.id);
                            }
                            commands::insert::insert_char(cx, c);
                        }
                    }))
                }
                CompleteAction::Selected { savepoint } => {
                    let (view, doc) = current!(editor);
                    doc.restore(view, &savepoint, false);
                }
            }
        }
        on_next_key
    }

    pub fn handle_idle_timeout(&mut self, cx: &mut commands::Context) -> EventResult {
        commands::compute_inlay_hints_for_all_views(cx.editor, cx.jobs);

        EventResult::Ignored(None)
    }
}

/// Whether the focused doc's workspace is in restricted mode and running `trust` would
/// change something visible at the workspace level.
fn workspace_trust_indicator_visible(editor: &Editor) -> bool {
    if editor.workspace_trust.implicit_level()
        == helix_loader::workspace_trust::ImplicitTrustLevel::Insecure
    {
        return false;
    }
    let (_, doc) = helix_view::current_ref!(editor);
    editor
        .workspace_trust
        .restricted_for_doc(doc.workspace_root(), doc.servers_to_load())
}

impl EditorView {
    /// must be called whenever the editor processed input that
    /// is not a `KeyEvent`. In these cases any pending keys/on next
    /// key callbacks must be canceled.
    fn handle_non_key_input(&mut self, cxt: &mut commands::Context) {
        cxt.editor.status_msg = None;
        cxt.editor.reset_idle_timer();
        // HACKS: create a fake key event that will never trigger any actual map
        // and therefore simply acts as "dismiss"
        let null_key_event = KeyEvent {
            code: KeyCode::Null,
            modifiers: KeyModifiers::empty(),
        };
        // dismiss any pending keys
        if let Some((on_next_key, _)) = self.on_next_key.take() {
            on_next_key(cxt, null_key_event);
        }
        self.handle_keymap_event(cxt.editor.mode, cxt, null_key_event);
        self.pseudo_pending.clear();
    }

    fn handle_mouse_event(
        &mut self,
        event: &MouseEvent,
        cxt: &mut commands::Context,
    ) -> EventResult {
        if event.kind != MouseEventKind::Moved {
            self.handle_non_key_input(cxt)
        }

        let config = cxt.editor.config();
        let MouseEvent {
            kind,
            row,
            column,
            modifiers,
            ..
        } = *event;

        let pos_and_view = |editor: &Editor, row, column, ignore_virtual_text| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.pos_at_screen_coords(
                    &editor.documents[&view.doc],
                    row,
                    column,
                    ignore_virtual_text,
                )
                .map(|pos| (pos, view.id))
            })
        };

        let gutter_coords_and_view = |editor: &Editor, row, column| {
            editor.tree.views().find_map(|(view, _focus)| {
                view.gutter_coords_at_screen_coords(row, column)
                    .map(|coords| (coords, view.id))
            })
        };

        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let editor = &mut cxt.editor;

                if let Some((pos, view_id)) = pos_and_view(editor, row, column, true) {
                    editor.focus(view_id);

                    let prev_view_id = view!(editor).id;
                    let doc = doc_mut!(editor, &view!(editor, view_id).doc);

                    if modifiers == KeyModifiers::ALT {
                        let selection = doc.selection(view_id).clone();
                        doc.set_selection(view_id, selection.push(Range::point(pos)));
                    } else if editor.mode == Mode::Select {
                        // Discards non-primary selections for consistent UX with normal mode
                        let primary = doc.selection(view_id).primary().put_cursor(
                            doc.text().slice(..),
                            pos,
                            true,
                        );
                        editor.mouse_down_range = Some(primary);
                        doc.set_selection(view_id, Selection::single(primary.anchor, primary.head));
                    } else {
                        doc.set_selection(view_id, Selection::point(pos));
                    }

                    if view_id != prev_view_id {
                        self.clear_completion(editor);
                    }

                    editor.ensure_cursor_in_view(view_id);

                    return EventResult::Consumed(None);
                }

                if let Some((coords, view_id)) = gutter_coords_and_view(editor, row, column) {
                    editor.focus(view_id);

                    let (view, doc) = current!(cxt.editor);

                    let Some(path) = doc.path().map(ToOwned::to_owned) else {
                        return EventResult::Ignored(None);
                    };

                    if let Some(char_idx) =
                        view.pos_at_visual_coords(doc, coords.row as u16, coords.col as u16, true)
                    {
                        let line = doc.text().char_to_line(char_idx);
                        commands::dap_toggle_breakpoint_impl(cxt, path, line);
                        return EventResult::Consumed(None);
                    }
                }

                EventResult::Ignored(None)
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                let (view, doc) = current!(cxt.editor);

                let pos = match view.pos_at_screen_coords(doc, row, column, true) {
                    Some(pos) => pos,
                    None => return EventResult::Ignored(None),
                };

                let mut selection = doc.selection(view.id).clone();
                let primary = selection.primary_mut();
                *primary = primary.put_cursor(doc.text().slice(..), pos, true);
                doc.set_selection(view.id, selection);
                let view_id = view.id;
                cxt.editor.ensure_cursor_in_view(view_id);
                EventResult::Consumed(None)
            }

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let current_view = cxt.editor.tree.focus;

                let direction = match event.kind {
                    MouseEventKind::ScrollUp => Direction::Backward,
                    MouseEventKind::ScrollDown => Direction::Forward,
                    _ => unreachable!(),
                };

                match pos_and_view(cxt.editor, row, column, false) {
                    Some((_, view_id)) => cxt.editor.tree.focus = view_id,
                    None => return EventResult::Ignored(None),
                }

                let offset = config.scroll_lines.unsigned_abs();
                commands::scroll(cxt, offset, direction, false);

                cxt.editor.tree.focus = current_view;
                cxt.editor.ensure_cursor_in_view(current_view);

                EventResult::Consumed(None)
            }

            MouseEventKind::Up(MouseButton::Left) => {
                if !config.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                let (view, doc) = current!(cxt.editor);

                let should_yank = match cxt.editor.mouse_down_range.take() {
                    Some(down_range) => doc.selection(view.id).primary() != down_range,
                    None => {
                        // This should not happen under normal cases. We fall back to the original
                        // behavior of yanking on non-single-char selections.
                        doc.selection(view.id)
                            .primary()
                            .slice(doc.text().slice(..))
                            .len_chars()
                            > 1
                    }
                };

                if should_yank {
                    commands::yank_main_selection_to_register(
                        cxt.editor,
                        config.mouse_yank_register,
                    );
                    EventResult::Consumed(None)
                } else {
                    EventResult::Ignored(None)
                }
            }

            MouseEventKind::Up(MouseButton::Right) => {
                if let Some((pos, view_id)) = gutter_coords_and_view(cxt.editor, row, column) {
                    cxt.editor.focus(view_id);

                    if let Some((pos, _)) = pos_and_view(cxt.editor, row, column, true) {
                        doc_mut!(cxt.editor).set_selection(view_id, Selection::point(pos));
                    } else {
                        let (view, doc) = current!(cxt.editor);

                        if let Some(pos) = view.pos_at_visual_coords(doc, pos.row as u16, 0, true) {
                            doc.set_selection(view_id, Selection::point(pos));
                            match modifiers {
                                KeyModifiers::ALT => {
                                    commands::MappableCommand::dap_edit_log.execute(cxt)
                                }
                                _ => commands::MappableCommand::dap_edit_condition.execute(cxt),
                            };
                        }
                    }

                    cxt.editor.ensure_cursor_in_view(view_id);
                    return EventResult::Consumed(None);
                }
                EventResult::Ignored(None)
            }

            MouseEventKind::Up(MouseButton::Middle) => {
                let editor = &mut cxt.editor;
                if !config.middle_click_paste {
                    return EventResult::Ignored(None);
                }

                if modifiers == KeyModifiers::ALT {
                    commands::replace_selections_with_register(
                        cxt.editor,
                        config.mouse_yank_register,
                        cxt.count(),
                    );

                    return EventResult::Consumed(None);
                }

                if let Some((pos, view_id)) = pos_and_view(editor, row, column, true) {
                    let doc = doc_mut!(editor, &view!(editor, view_id).doc);
                    doc.set_selection(view_id, Selection::point(pos));
                    cxt.editor.focus(view_id);

                    commands::paste(
                        cxt.editor,
                        config.mouse_yank_register,
                        commands::Paste::Before,
                        cxt.count(),
                    );

                    return EventResult::Consumed(None);
                }

                EventResult::Ignored(None)
            }

            _ => EventResult::Ignored(None),
        }
    }
    /// Make the next key go to `callback` (a command run by a plugin asked
    /// for it, like `r` does).
    pub(crate) fn set_on_next_key(
        &mut self,
        callback: commands::OnKeyCallback,
        kind: commands::OnKeyCallbackKind,
    ) {
        self.on_next_key = Some((callback, kind));
    }

    fn on_next_key(
        &mut self,
        kind: OnKeyCallbackKind,
        ctx: &mut commands::Context,
        event: KeyEvent,
    ) -> bool {
        if let Some((on_next_key, kind_)) = self.on_next_key.take() {
            if kind == kind_ {
                on_next_key(ctx, event);
                true
            } else {
                self.on_next_key = Some((on_next_key, kind_));
                false
            }
        } else {
            false
        }
    }
}

impl Component for EditorView {
    fn handle_event(
        &mut self,
        event: &Event,
        context: &mut crate::compositor::Context,
    ) -> EventResult {
        let mut cx = commands::Context {
            editor: context.editor,
            count: None,
            register: None,
            callback: Vec::new(),
            on_next_key_callback: None,
            jobs: context.jobs,
        };

        match event {
            Event::Paste(contents) => {
                self.handle_non_key_input(&mut cx);
                cx.count = cx.editor.count;
                commands::paste_bracketed_value(&mut cx, contents.clone());
                cx.editor.count = None;

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);
                view.ensure_cursor_in_view(doc, config.scrolloff);

                // Store a history state if not in insert mode. Otherwise wait till we exit insert
                // to include any edits to the paste in the history state.
                if mode != Mode::Insert {
                    doc.append_changes_to_history(view);
                }

                EventResult::Consumed(None)
            }
            Event::Resize(_width, _height) => {
                // Ignore this event, we handle resizing just before rendering to screen.
                // Handling it here but not re-rendering will cause flashing
                EventResult::Consumed(None)
            }
            Event::Key(mut key) => {
                cx.editor.reset_idle_timer();
                canonicalize_key(&mut key);

                // A focused panel takes its keys.
                let focused = cx.editor.panels.iter().find(|p| p.focused).map(|p| p.dock);
                if let Some(dock) = focused.filter(|_| self.pseudo_pending.is_empty()) {
                    let rows = self
                        .panel_areas
                        .iter()
                        .find(|(d, _)| *d == dock)
                        .map_or(0, |(d, area)| crate::ui::sidebar::rows(*d, *area));
                    if crate::ui::sidebar::handle_key(cx.editor, cx.jobs, key, rows) {
                        return EventResult::Consumed(None);
                    }
                }

                // clear status
                cx.editor.status_msg = None;

                let mode = cx.editor.mode();

                if !self.on_next_key(OnKeyCallbackKind::PseudoPending, &mut cx, key) {
                    match mode {
                        Mode::Insert => {
                            // let completion swallow the event if necessary
                            let mut consumed = false;
                            // Keys bound to a plugin aren't typing: they go to
                            // the plugin, which may drive the menu itself.
                            let plugin_key = crate::keymap::plugin_layers::has_bindings() && {
                                let doc = doc!(cx.editor);
                                let language = doc.language_name().map(str::to_string);
                                let scope = crate::keymap::plugin_layers::KeyScope {
                                    owner: doc.plugin_owner.as_deref(),
                                    document: Some(doc.id()),
                                    language: language.as_deref(),
                                };
                                self.keymaps.is_plugin_binding(Mode::Insert, key, &scope)
                            };
                            // `completion-keys`: what the user bound for the open menu.
                            let menu_key = self
                                .completion
                                .as_ref()
                                .filter(|_| !plugin_key)
                                .and_then(|_| {
                                    cx.editor.config().completion_keys.get(&key).copied()
                                });
                            if let Some(action) = menu_key {
                                self.completion_key(action, &mut cx);
                                consumed = true;
                            }
                            if let Some(completion) = self
                                .completion
                                .as_mut()
                                .filter(|_| !plugin_key && menu_key.is_none())
                            {
                                let label = completion.selected_label();
                                let res = {
                                    // use a fake context here
                                    let mut cx = Context {
                                        editor: cx.editor,
                                        jobs: cx.jobs,
                                        scroll: None,
                                    };

                                    if let EventResult::Consumed(callback) =
                                        completion.handle_event(event, &mut cx)
                                    {
                                        consumed = true;
                                        Some(callback)
                                    } else if let EventResult::Consumed(callback) =
                                        completion.handle_event(&Event::Key(key!(Enter)), &mut cx)
                                    {
                                        Some(callback)
                                    } else {
                                        None
                                    }
                                };

                                if let Some(callback) = res {
                                    if callback.is_some() {
                                        // assume close_fn: aborted with esc or ctrl-c,
                                        // else an item was accepted.
                                        use crate::ui::completion::menu_events::{
                                            set_close_reason, Close, MenuEvent,
                                        };
                                        if consumed
                                            && (key == key!(Esc) || key == crate::ctrl!('c'))
                                        {
                                            set_close_reason(Close::Aborted);
                                        } else {
                                            if let Some(label) = label {
                                                crate::plugins::completion_menu_event(
                                                    cx.editor,
                                                    MenuEvent::Confirmed(label),
                                                );
                                            }
                                            set_close_reason(Close::Confirmed);
                                        }
                                        if let Some(cb) = self.clear_completion(cx.editor) {
                                            if consumed {
                                                cx.on_next_key_callback =
                                                    Some((cb, OnKeyCallbackKind::Fallback))
                                            } else {
                                                self.on_next_key =
                                                    Some((cb, OnKeyCallbackKind::Fallback));
                                            }
                                        }
                                    } else {
                                        self.sync_menu(cx.editor);
                                    }
                                }
                            }

                            // if completion didn't take the event, we pass it onto commands.
                            // Ctrl-c closes the menu and still does what it is bound to
                            // (leave insert mode), so one press is enough.
                            if !consumed || key == crate::ctrl!('c') {
                                self.insert_mode(&mut cx, key);

                                // record last_insert key
                                self.last_insert.1.push(InsertEvent::Key(key));
                            }
                        }
                        mode => self.command_mode(mode, &mut cx, key),
                    }
                }

                self.on_next_key = cx.on_next_key_callback.take();
                match self.on_next_key {
                    Some((_, OnKeyCallbackKind::PseudoPending)) => self.pseudo_pending.push(key),
                    _ => self.pseudo_pending.clear(),
                }

                // appease borrowck
                let callbacks = take(&mut cx.callback);

                // if the command consumed the last view, skip the render.
                // on the next loop cycle the Application will then terminate.
                if cx.editor.should_close() {
                    return EventResult::Ignored(None);
                }

                let config = cx.editor.config();
                let mode = cx.editor.mode();
                let (view, doc) = current!(cx.editor);

                view.ensure_cursor_in_view(doc, config.scrolloff);

                // Store a history state if not in insert mode. This also takes care of
                // committing changes when leaving insert mode.
                if mode != Mode::Insert {
                    doc.append_changes_to_history(view);
                }
                let callback = if callbacks.is_empty() {
                    None
                } else {
                    let callback: crate::compositor::Callback = Box::new(move |compositor, cx| {
                        for callback in callbacks {
                            callback(compositor, cx)
                        }
                    });
                    Some(callback)
                };

                EventResult::Consumed(callback)
            }

            Event::Mouse(event) => {
                if crate::ui::sidebar::handle_mouse(cx.editor, cx.jobs, event, &self.panel_areas) {
                    return EventResult::Consumed(None);
                }
                // A click elsewhere gives the editor its keys back.
                if matches!(event.kind, MouseEventKind::Down(_)) {
                    cx.editor.panels.iter_mut().for_each(|p| p.focused = false);
                }
                self.handle_mouse_event(event, &mut cx)
            }
            Event::IdleTimeout => self.handle_idle_timeout(&mut cx),
            Event::FocusGained => {
                self.terminal_focused = true;
                EventResult::Consumed(None)
            }
            Event::FocusLost => {
                if context.editor.config().auto_save.focus_lost {
                    let options = commands::WriteAllOptions {
                        force: false,
                        write_scratch: false,
                        auto_format: false,
                        code_actions: false,
                    };
                    if let Err(e) = commands::typed::write_all_impl(context, options) {
                        context.editor.set_error(format!("{}", e));
                    }
                }
                self.terminal_focused = false;
                EventResult::Consumed(None)
            }
        }
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        // clear with background color
        surface.set_style(area, cx.editor.theme.get("ui.background"));
        let config = cx.editor.config();

        // check if bufferline should be rendered
        use helix_view::editor::BufferLine;
        let use_bufferline = match config.bufferline {
            BufferLine::Always => true,
            BufferLine::Multiple if cx.editor.documents.len() > 1 => true,
            _ => false,
        };

        // -1 for commandline and -1 for bufferline
        let mut editor_area = area.clip_bottom(1);
        if use_bufferline {
            editor_area = editor_area.clip_top(1);
        }

        // Plugin panels take columns beside the views and rows below them.
        crate::plugins::check_focus(cx.editor);
        crate::plugins::set_pending_keys(self.keymaps.pending());
        let (views_area, panel_areas) = if cx.editor.panels.is_empty() {
            (editor_area, Vec::new())
        } else {
            crate::ui::sidebar::layout(cx.editor, editor_area)
        };
        editor_area = views_area;
        self.panel_areas = panel_areas;

        // if the terminal size suddenly changed, we need to trigger a resize
        cx.editor.resize(editor_area);

        if use_bufferline {
            Self::render_bufferline(cx.editor, area.with_height(1), surface);
        }

        for (view, is_focused) in cx.editor.tree.views() {
            let doc = cx.editor.document(view.doc).unwrap();
            self.render_view(cx.editor, doc, view, area, surface, is_focused);
        }

        for &(dock, area) in &self.panel_areas {
            crate::ui::sidebar::render(cx.editor, dock, area, surface);
        }

        if let Some(area) = Self::splash_area(cx.editor) {
            Self::render_splash(cx.editor, area, surface);
        }

        crate::commands::substitute_preview::render(cx.editor, editor_area, surface);

        if config.auto_info {
            if let Some(mut info) = cx.editor.autoinfo.take() {
                info.render(area, surface, cx);
                cx.editor.autoinfo = Some(info)
            }
        }

        let key_width = 15u16; // for showing pending keys
        let mut status_msg_width = 0;

        // render status msg
        if let Some((status_msg, severity)) = &cx.editor.status_msg {
            status_msg_width = status_msg.width();
            use helix_view::editor::Severity;
            let style = if *severity == Severity::Error {
                cx.editor.theme.get("error")
            } else {
                cx.editor.theme.get("ui.text")
            };

            surface.set_string(
                area.x,
                area.y + area.height.saturating_sub(1),
                status_msg,
                style,
            );
        }

        if area.width.saturating_sub(status_msg_width as u16) > key_width {
            let mut disp = String::new();
            if let Some(count) = cx.editor.count {
                disp.push_str(&count.to_string())
            }
            for key in self.keymaps.pending() {
                disp.push_str(&key.key_sequence_format());
            }
            for key in &self.pseudo_pending {
                disp.push_str(&key.key_sequence_format());
            }
            let style = cx.editor.theme.get("ui.text");
            let macro_width = if cx.editor.macro_recording.is_some() {
                3
            } else {
                0
            };
            let restricted = workspace_trust_indicator_visible(cx.editor);
            let trust_width = if restricted { 3 } else { 0 };
            surface.set_string(
                area.x
                    + area
                        .width
                        .saturating_sub(key_width + macro_width + trust_width),
                area.y + area.height.saturating_sub(1),
                disp.get(disp.len().saturating_sub(key_width as usize)..)
                    .unwrap_or(&disp),
                style,
            );
            if restricted {
                let style = style
                    .fg(helix_view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                surface.set_string(
                    area.x
                        .saturating_add(area.width.saturating_sub(3 + macro_width)),
                    area.y + area.height.saturating_sub(1),
                    "[⚠]",
                    style,
                );
            }
            if let Some((reg, _)) = cx.editor.macro_recording {
                let disp = format!("[{}]", reg);
                let style = style
                    .fg(helix_view::graphics::Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                surface.set_string(
                    area.x + area.width.saturating_sub(3),
                    area.y + area.height.saturating_sub(1),
                    &disp,
                    style,
                );
            }
        }

        if let Some(completion) = self.completion.as_mut() {
            completion.render(area, surface, cx);
        }
    }

    fn cursor(&self, _area: Rect, editor: &Editor) -> (Option<Position>, CursorKind) {
        match editor.cursor() {
            // all block cursors are drawn manually
            (pos, CursorKind::Block) => {
                if self.terminal_focused {
                    (pos, CursorKind::Hidden)
                } else {
                    // use terminal cursor when terminal loses focus
                    (pos, CursorKind::Underline)
                }
            }
            cursor => cursor,
        }
    }
}

fn canonicalize_key(key: &mut KeyEvent) {
    if let KeyEvent {
        code: KeyCode::Char(_),
        modifiers: _,
    } = key
    {
        key.modifiers.remove(KeyModifiers::SHIFT)
    }
}
