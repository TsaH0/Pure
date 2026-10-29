//! Example plugin: an outline of the current file in a panel on the right,
//! like aerial.nvim. Functions, types, classes, headings... found with the
//! editor's tree-sitter symbol queries (`tags.scm`, shipped for ~80
//! languages), or line by line where there is none; indented by nesting.
//! No language server needed. The symbol the cursor is in is selected as
//! you move; enter jumps to a symbol.
//!
//! `toggle` (`space O`) shows or hides it, `focus` gives it the keys.

use std::cell::RefCell;

use helix_plugin_api::{
    editor,
    panels::{self, item, on_enter, Dock},
    register_plugin, syntax, text, CommandInfo, DocumentId, EventKind, Plugin, Range,
    Registration,
};

#[derive(Debug, Clone, PartialEq)]
struct Symbol {
    /// 0-based line.
    line: u32,
    kind: &'static str,
    name: String,
    depth: u32,
}

#[derive(Default)]
struct State {
    shown: bool,
    doc: Option<DocumentId>,
    symbols: Vec<Symbol>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The identifier at the start of `s`.
fn ident(s: &str) -> Option<String> {
    let name: String = s
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// `line` without its indentation, and the indentation's width.
fn indent(line: &str) -> (usize, &str) {
    let trimmed = line.trim_start();
    let width = line[..line.len() - trimmed.len()]
        .chars()
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    (width, trimmed)
}

/// After any of `keywords` (and the modifiers before them), the name.
fn after_keyword(line: &str, keywords: &[(&str, &'static str)], modifiers: &[&str]) -> Option<(&'static str, String)> {
    let mut rest = line;
    loop {
        let before = rest;
        for modifier in modifiers {
            if let Some(r) = rest.strip_prefix(modifier).filter(|r| r.starts_with([' ', '('])) {
                // `pub(crate) fn`: skip the parenthesis too.
                rest = r.trim_start();
                if let Some(r) = rest.strip_prefix('(') {
                    rest = r.split_once(')').map_or("", |(_, r)| r).trim_start();
                }
            }
        }
        if rest == before {
            break;
        }
    }
    for (keyword, kind) in keywords {
        if let Some(r) = rest.strip_prefix(keyword) {
            if r.starts_with(' ') {
                let name = ident(r.trim_start().trim_start_matches(['*', '&']))?;
                return Some((kind, name));
            }
        }
    }
    None
}

/// `type name(args) {`: a C-like function definition's name.
fn c_function(code: &str) -> Option<String> {
    let open = code.find('(')?;
    let head = &code[..open];
    let last = head.split_whitespace().last()?;
    let name = last.trim_start_matches(['*', '&']).rsplit("::").next()?.to_string();
    let keywords = ["if", "for", "while", "switch", "return", "else", "catch", "sizeof", "new", "delete"];
    (head.split_whitespace().count() >= 2
        && !keywords.contains(&name.as_str())
        && !code.ends_with(';')
        && ident(&name).as_deref() == Some(name.as_str()))
    .then_some(name)
}

/// The symbols of `source`, written in `language`.
fn symbols(language: &str, source: &str) -> Vec<Symbol> {
    let mut out = Vec::new();
    // Indentation of the enclosing symbols, to nest by.
    let mut stack: Vec<usize> = Vec::new();
    let mut in_fence = false;
    for (number, line) in source.lines().enumerate() {
        let (width, code) = indent(line);
        let found: Option<(&'static str, String, Option<u32>)> = match language {
            "markdown" => {
                if code.starts_with("```") || code.starts_with("~~~") {
                    in_fence = !in_fence;
                }
                let level = line.chars().take_while(|c| *c == '#').count();
                (!in_fence && (1..=6).contains(&level) && line[level..].starts_with(' '))
                    .then(|| ("#", line[level..].trim().to_string(), Some(level as u32 - 1)))
            }
            "rust" => after_keyword(
                code,
                &[
                    ("fn", "fn"),
                    ("struct", "struct"),
                    ("enum", "enum"),
                    ("trait", "trait"),
                    ("mod", "mod"),
                    ("type", "type"),
                    ("const", "const"),
                    ("static", "static"),
                    ("macro_rules!", "macro"),
                ],
                &["pub", "async", "unsafe", "const", "extern \"C\"", "default"],
            )
            .or_else(|| {
                code.strip_prefix("impl")
                    .filter(|r| r.starts_with([' ', '<']))
                    .map(|r| {
                        let r = if r.starts_with('<') { r.split_once('>').map_or(r, |(_, r)| r) } else { r };
                        ("impl", r.trim().trim_end_matches('{').trim().to_string())
                    })
            })
            .map(|(k, n)| (k, n, None)),
            "python" => after_keyword(code, &[("def", "def"), ("class", "class")], &["async"])
                .map(|(k, n)| (k, n, None)),
            "javascript" | "typescript" | "tsx" | "jsx" => after_keyword(
                code,
                &[
                    ("function", "function"),
                    ("class", "class"),
                    ("interface", "interface"),
                    ("type", "type"),
                    ("enum", "enum"),
                ],
                &["export", "default", "async", "declare", "abstract"],
            )
            .or_else(|| {
                // `const name = (…) =>` / `= function`
                let rest = code.strip_prefix("export ").unwrap_or(code);
                let rest = ["const ", "let ", "var "].iter().find_map(|k| rest.strip_prefix(k))?;
                let name = ident(rest)?;
                let value = rest[name.len()..].trim_start().strip_prefix('=')?.trim_start();
                (value.contains("=>") || value.starts_with("function") || value.starts_with("async"))
                    .then_some(("function", name))
            })
            .map(|(k, n)| (k, n, None)),
            "go" => code
                .strip_prefix("func ")
                .map(|r| {
                    // `func (r *T) Name(`
                    let r = if r.starts_with('(') { r.split_once(')').map_or(r, |(_, r)| r).trim_start() } else { r };
                    ident(r).map(|n| ("func", n))
                })
                .flatten()
                .or_else(|| after_keyword(code, &[("type", "type")], &[]))
                .map(|(k, n)| (k, n, None)),
            "lua" => after_keyword(code, &[("function", "function")], &["local"])
                .map(|(k, n)| {
                    // `function M.name(`
                    let full: String = code
                        .split_once("function ")
                        .map(|(_, r)| r.chars().take_while(|c| *c != '(').collect())
                        .unwrap_or(n);
                    (k, full.trim().to_string(), None)
                }),
            "c" | "cpp" | "java" | "c-sharp" => {
                if let Some((k, n)) = after_keyword(code, &[("class", "class"), ("struct", "struct"), ("enum", "enum"), ("interface", "interface"), ("namespace", "namespace")], &["public", "private", "protected", "static", "final", "abstract", "typedef"]) {
                    Some((k, n, None))
                } else {
                    c_function(code).map(|name| ("fn", name, None))
                }
            }
            "toml" => code
                .strip_prefix('[')
                .and_then(|r| r.split_once(']'))
                .map(|(name, _)| ("[]", name.trim_matches(['[', ']']).to_string(), None)),
            _ => None,
        };
        // Nesting: symbols indented under the one before.
        while stack.last().is_some_and(|w| *w >= width) && !code.is_empty() {
            stack.pop();
        }
        if let Some((kind, name, depth)) = found {
            let depth = depth.unwrap_or(stack.len() as u32);
            out.push(Symbol { line: number as u32, kind, name, depth });
            if language != "markdown" {
                stack.push(width);
            }
        }
    }
    out
}

/// The symbols of `doc` from its syntax tree and the language's `tags.scm`
/// (`@definition.function` around `@name`...), nested by containment. None
/// without a tree or a tags query.
fn tree_symbols(doc: DocumentId, language: &str, source: &str) -> Option<Vec<Symbol>> {
    let mut tags = syntax::query_file(language, "tags.scm")?;
    if language == "rust" {
        // Methods nest under their `impl`, which tags.scm leaves out.
        tags.push_str("\n(impl_item type: (_) @name) @definition.impl\n");
    }
    let matches = syntax::query(doc, &tags, None).ok()?;
    let starts = text::line_starts(source);
    let line_of = |pos: u32| (starts.partition_point(|start| *start <= pos) as u32).saturating_sub(1);
    // (start, end, kind, name), in document order.
    let mut found: Vec<(u32, u32, &'static str, String)> = matches
        .iter()
        .filter_map(|found| {
            let definition = found.captures.iter().find(|c| c.name.starts_with("definition."))?;
            let name = found.captures.iter().find(|c| c.name == "name")?;
            let kind = match &definition.name["definition.".len()..] {
                "function" | "method" => "fn",
                "impl" => "impl",
                "macro" => "macro",
                "module" | "namespace" => "mod",
                "constant" => "const",
                "struct" => "struct",
                "enum" => "enum",
                "interface" | "trait" => "trait",
                "class" => "class",
                "type" => "type",
                _ => return None,
            };
            let name: String = source
                .chars()
                .skip(name.node.start as usize)
                .take((name.node.end - name.node.start) as usize)
                .collect();
            Some((definition.node.start, definition.node.end, kind, name))
        })
        .collect();
    found.sort_by_key(|(start, end, ..)| (*start, std::cmp::Reverse(*end)));
    found.dedup_by_key(|(start, ..)| *start);
    let mut out = Vec::new();
    // Ends of the enclosing definitions.
    let mut stack: Vec<u32> = Vec::new();
    for (start, end, kind, name) in found {
        while stack.last().is_some_and(|outer| *outer <= start) {
            stack.pop();
        }
        out.push(Symbol { line: line_of(start), kind, name, depth: stack.len() as u32 });
        stack.push(end);
    }
    Some(out)
}

/// The symbol the cursor's `line` is in: the last one starting at or before it.
fn symbol_at(symbols: &[Symbol], line: u32) -> Option<usize> {
    symbols.iter().rposition(|s| s.line <= line)
}

/// Read the current document again and show its outline.
fn refresh() -> Result<(), String> {
    if !STATE.with_borrow(|s| s.shown) {
        return Ok(());
    }
    let Some(doc) = editor::current_document() else {
        return Ok(());
    };
    // Plugin buffers (ours aside) and the like have nothing to outline.
    let language = editor::document_language(doc)?.unwrap_or_default();
    let source = editor::text(doc)?;
    let found = tree_symbols(doc, &language, &source).unwrap_or_else(|| symbols(&language, &source));
    STATE.with_borrow_mut(|state| {
        state.doc = Some(doc);
        state.symbols = found;
    });
    show(doc, &source)
}

fn cursor_line(doc: DocumentId, source: &str) -> u32 {
    let Some(head) = editor::selections(doc).ok().and_then(|s| s.first().map(|r| r.head)) else {
        return 0;
    };
    let byte = text::char_to_byte(source, head);
    source[..byte].matches('\n').count() as u32
}

fn show(doc: DocumentId, source: &str) -> Result<(), String> {
    let line = cursor_line(doc, source);
    let (items, selected) = STATE.with_borrow(|state| {
        let items: Vec<_> = state
            .symbols
            .iter()
            .map(|s| {
                let scope = match s.kind {
                    "fn" | "def" | "function" | "func" | "macro" => "function",
                    "#" => "markup.heading",
                    "impl" | "mod" | "namespace" | "[]" => "namespace",
                    "trait" | "class" | "interface" => "type",
                    "const" | "static" => "constant",
                    _ => "type",
                };
                on_enter(item(format!("{} {}", s.kind, s.name), scope, s.depth), "jump", vec![s.line.to_string()])
            })
            .collect();
        (items, symbol_at(&state.symbols, line))
    });
    let name = editor::document_path(doc)
        .ok()
        .flatten()
        .and_then(|p| p.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| "[scratch]".into());
    let mut panel = panels::panel(Dock::Right, &format!("Outline: {name}"), items, 32);
    panel.selected = selected.map(|s| s as u32);
    panels::show(&panel)
}

/// Put the cursor on `line` of the outlined document.
fn jump(line: u32) -> Result<(), String> {
    let doc = STATE.with_borrow(|s| s.doc).ok_or("no outline")?;
    let source = editor::text(doc)?;
    let starts = text::line_starts(&source);
    let pos = *starts.get(line as usize).ok_or("the line is gone")?;
    editor::set_selections(doc, &[Range { anchor: pos, head: pos }])?;
    panels::focus(Dock::Right, false);
    Ok(())
}

struct Outline;

impl Plugin for Outline {
    fn init(_config: String) -> Result<Registration, String> {
        let command = |name: &str, doc: &str| CommandInfo { name: name.into(), doc: doc.into() };
        Ok(Registration {
            commands: vec![
                command("toggle", "Show or hide the outline of the current file"),
                command("focus", "Give the outline the keys"),
                command("jump", "Go to a line of the outlined file"),
            ],
            events: vec![EventKind::DocumentChange, EventKind::DocumentOpen, EventKind::SelectionChange],
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "toggle" => {
                let shown = STATE.with_borrow_mut(|s| {
                    s.shown = !s.shown;
                    s.shown
                });
                if shown {
                    refresh()
                } else {
                    panels::close(Dock::Right);
                    Ok(())
                }
            }
            "focus" => {
                STATE.with_borrow_mut(|s| s.shown = true);
                refresh()?;
                panels::focus(Dock::Right, true);
                Ok(())
            }
            "jump" => jump(args.first().and_then(|l| l.parse().ok()).ok_or("which line?")?),
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_document_change(_doc: DocumentId) {
        let _ = refresh();
    }
    fn on_document_open(_doc: DocumentId) {
        let _ = refresh();
    }
    fn on_selection_change(doc: DocumentId) {
        // Only the selection moves (or the current document changed).
        if !STATE.with_borrow(|s| s.shown) {
            return;
        }
        if STATE.with_borrow(|s| s.doc) != Some(doc) || editor::current_document() != Some(doc) {
            let _ = refresh();
            return;
        }
        if let Ok(source) = editor::text(doc) {
            let _ = show(doc, &source);
        }
    }
}

register_plugin!(Outline);

#[cfg(test)]
mod test {
    use super::*;

    fn names(language: &str, source: &str) -> Vec<(u32, String, u32)> {
        symbols(language, source)
            .into_iter()
            .map(|s| (s.line, format!("{} {}", s.kind, s.name), s.depth))
            .collect()
    }

    #[test]
    fn rust_symbols_nest_by_indentation() {
        let source = "use x;\npub struct A;\nimpl<T> Foo for A {\n    pub(crate) async fn go(&self) {}\n    fn other() {}\n}\nfn main() {}\n";
        assert_eq!(
            names("rust", source),
            [
                (1, "struct A".to_string(), 0),
                (2, "impl Foo for A".to_string(), 0),
                (3, "fn go".to_string(), 1),
                (4, "fn other".to_string(), 1),
                (6, "fn main".to_string(), 0),
            ]
        );
    }

    #[test]
    fn other_languages() {
        assert_eq!(
            names("python", "class A:\n    def f(self):\n        pass\nasync def g():\n    pass\n"),
            [(0, "class A".into(), 0), (1, "def f".into(), 1), (3, "def g".into(), 0)]
        );
        assert_eq!(
            names("markdown", "# Title\ntext\n## Sub\n```\n# not\n```\n"),
            [(0, "# Title".into(), 0), (2, "# Sub".into(), 1)]
        );
        assert_eq!(
            names("go", "func (r *T) Name() {}\ntype T struct{}\n"),
            [(0, "func Name".into(), 0), (1, "type T".into(), 0)]
        );
        assert_eq!(
            names("javascript", "export function a() {}\nconst b = (x) => x;\nconst c = 3;\n"),
            [(0, "function a".into(), 0), (1, "function b".into(), 0)]
        );
        assert_eq!(
            names("cpp", "int main(int argc) {\n  if (x) {}\n  foo(1);\n}\nvoid f();\n"),
            [(0, "fn main".into(), 0)]
        );
    }

    #[test]
    fn the_cursor_is_in_the_last_symbol_before_it() {
        let s = symbols("rust", "fn a() {}\n\nfn b() {}\n");
        assert_eq!(symbol_at(&s, 0), Some(0));
        assert_eq!(symbol_at(&s, 1), Some(0));
        assert_eq!(symbol_at(&s, 2), Some(1));
    }
}
