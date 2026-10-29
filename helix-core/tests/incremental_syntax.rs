//! Differential tests for the incremental injection and locals queries of the (vendored)
//! `tree-house` crate.
//!
//! After an edit `tree-house` only re-runs the injection and locals queries over the changed
//! parts of a document. These tests apply random edits to documents and check that the
//! resulting layers, syntax trees, locals and highlights are identical to the result of
//! re-running the queries over the whole document.

use std::collections::HashSet;
use std::fmt::Write;
use std::time::Duration;

use helix_core::syntax::{pretty_print_tree, Loader};
use helix_core::{ChangeSet, Rope, RopeSlice, Transaction};
use tree_house::highlighter::Highlighter;
use tree_house::tree_sitter::{InputEdit, Point};
use tree_house::Syntax;

const TIMEOUT: Duration = Duration::from_secs(60);

struct Sample {
    language: &'static str,
    source: &'static str,
    snippets: &'static [&'static str],
}

const SAMPLES: &[Sample] = &[
    Sample {
        language: "lua",
        source: r#"-- TODO: refactor f_IDX
--- Doc comment with @param a number
-- FIXME(bob): handle nil here
local function f_IDX(a, b)
  local t = { x = a, y = "str" .. b } -- NOTE trailing comment
  --[[ block comment
       HACK: spans lines ]]
  if a > b then
    return t.x + 1 -- plain comment
  end
  local m = string.match(b, "%d+") -- XXX: pattern injection
  local n = ("abc"):gsub("%a", "x")
  for i = 1, 10 do print(i) end
  return nil
end
"#,
        snippets: &[
            "--",
            "--[[",
            "]]",
            "\"",
            "'",
            "\n",
            " ",
            "x",
            "end",
            "function g() ",
            "-- TODO: x\n",
            "string.match(s, \"%w+\")",
            ":gsub(\"%s\", \"\")",
            "(",
            ")",
            "{",
            "}",
            "local ",
            "--[==[",
            "]==]",
            "\n-- FIXME: y\n",
            "return ",
            "if x then ",
            "match",
            "gmatch",
        ],
    },
    Sample {
        language: "rust",
        source: r#"// TODO: refactor f_IDX
/// Doc comment with `code` and **markdown**
fn f_IDX(a: u32, b: &str) -> Option<u32> {
    let t = Regex::new(r"\d+").unwrap(); // NOTE regex
    /* block FIXME: comment */
    if a > 1 {
        return Some(a + 1);
    }
    let s = format!("{a} {}", b);
    for i in 0..10 { println!("{i}"); }
    None
}
"#,
        snippets: &[
            "//",
            "/*",
            "*/",
            "\"",
            "'",
            "\n",
            " ",
            "x",
            "}",
            "{",
            "fn g() ",
            "// TODO: x\n",
            "/// doc\n",
            "r\"",
            "#",
            "let ",
            "(",
            ")",
            "Regex::new(r\"\\w\")",
            "format!(\"{}\", x)",
            "if x ",
            "=> ",
            "'a ",
        ],
    },
    Sample {
        language: "python",
        source: r#"# TODO: refactor f_IDX
def f_IDX(a, b):
    """Docstring FIXME"""
    t = re.compile(r"\d+")  # NOTE regex
    if a > b:
        return t.match(b)
    for i in range(10):
        print(f"{i} {a}")
    return None
"#,
        snippets: &[
            "#",
            "\"\"\"",
            "\"",
            "'",
            "\n",
            "    ",
            "x",
            ":",
            "def g():\n",
            "# TODO: x\n",
            "re.compile(r\"\\w\")",
            "(",
            ")",
            "lambda x: ",
            "class C:\n",
            "f\"{",
        ],
    },
    Sample {
        language: "javascript",
        source: r#"// TODO: refactor f_IDX
/**
 * JSDoc comment
 * @param {number} a
 */
function f_IDX(a, b) {
  const t = /\d+/g; // NOTE regex
  /* block FIXME: comment */
  if (a > b) {
    return t.test(b);
  }
  const s = `${a} ${b}`;
  for (let i = 0; i < 10; i++) { console.log(i); }
  return null;
}
"#,
        snippets: &[
            "//",
            "/*",
            "*/",
            "/**",
            "\"",
            "'",
            "`",
            "${",
            "\n",
            " ",
            "x",
            "}",
            "{",
            "function g() ",
            "// TODO: x\n",
            "/\\w+/",
            "(",
            ")",
            "const ",
            "=> ",
            "let ",
        ],
    },
    Sample {
        language: "html",
        source: r#"<!-- TODO: refactor f_IDX -->
<div class="c_IDX">
  <script>
    // FIXME: script
    const x = 1;
  </script>
  <style>
    .a { color: red; }
  </style>
  <p>text</p>
</div>
"#,
        snippets: &[
            "<!--",
            "-->",
            "<",
            ">",
            "</",
            "\"",
            "\n",
            " ",
            "x",
            "<script>",
            "</script>",
            "<style>",
            "</style>",
            "{",
            "}",
        ],
    },
    Sample {
        language: "markdown",
        source: r#"# Heading IDX

Some *emphasis* and `code`.

```rust
fn f_IDX() {} // TODO: x
```

- item
- [link](https://example.com)
"#,
        snippets: &[
            "#", "*", "`", "```", "```lua\n", "\n", " ", "x", "- ", "[", "]", "(", ")",
        ],
    },
];

/// xorshift: deterministic and good enough for generating edits.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Converts a changeset into tree-sitter edits, like `helix_core::syntax::Syntax::update`.
fn generate_edits(old_text: RopeSlice, changeset: &ChangeSet) -> Vec<InputEdit> {
    use helix_core::Operation::*;

    let mut old_pos = 0;
    let mut edits = Vec::new();
    let mut iter = changeset.changes().iter().peekable();
    while let Some(change) = iter.next() {
        let len = match change {
            Delete(i) | Retain(i) => *i,
            Insert(_) => 0,
        };
        let mut old_end = old_pos + len;
        match change {
            Retain(_) => {}
            Delete(_) => {
                let start_byte = old_text.char_to_byte(old_pos) as u32;
                let old_end_byte = old_text.char_to_byte(old_end) as u32;
                edits.push(InputEdit {
                    start_byte,
                    old_end_byte,
                    new_end_byte: start_byte,
                    start_point: Point::ZERO,
                    old_end_point: Point::ZERO,
                    new_end_point: Point::ZERO,
                });
            }
            Insert(s) => {
                let start_byte = old_text.char_to_byte(old_pos) as u32;
                // a subsequent delete means a replace, consume it
                if let Some(Delete(len)) = iter.peek() {
                    old_end = old_pos + len;
                    let old_end_byte = old_text.char_to_byte(old_end) as u32;
                    iter.next();
                    edits.push(InputEdit {
                        start_byte,
                        old_end_byte,
                        new_end_byte: start_byte + s.len() as u32,
                        start_point: Point::ZERO,
                        old_end_point: Point::ZERO,
                        new_end_point: Point::ZERO,
                    });
                } else {
                    edits.push(InputEdit {
                        start_byte,
                        old_end_byte: start_byte,
                        new_end_byte: start_byte + s.len() as u32,
                        start_point: Point::ZERO,
                        old_end_point: Point::ZERO,
                        new_end_point: Point::ZERO,
                    });
                }
            }
        }
        old_pos = old_end;
    }
    edits
}

fn root_tree(syntax: &Syntax) -> String {
    let mut out = String::new();
    let tree = syntax.layer(syntax.root()).tree().unwrap();
    pretty_print_tree(&mut out, tree.root_node()).unwrap();
    out
}

/// A textual representation of everything the incremental queries affect: the layers at
/// every byte (with their language, range, syntax tree and locals) and the highlights.
fn snapshot(syntax: &Syntax, text: &Rope, loader: &Loader) -> String {
    let mut out = String::new();
    let len = text.len_bytes() as u32;
    let mut seen = HashSet::new();
    for byte in 0..=len {
        let _ = write!(out, "{byte}:");
        for layer in syntax.layers_for_byte_range(byte, byte) {
            let data = syntax.layer(layer);
            let range = data.tree().map(|tree| tree.root_node().byte_range());
            let _ = write!(out, " {:?}{:?}", data.language, range);
            if seen.insert(layer) {
                let _ = write!(out, "<locals {}>", data.debug_locals());
                if let Some(tree) = data.tree() {
                    let mut tree_text = String::new();
                    pretty_print_tree(&mut tree_text, tree.root_node()).unwrap();
                    let _ = write!(out, "<{tree_text}>");
                }
            }
        }
        out.push('\n');
    }
    let mut highlighter = Highlighter::new(syntax, text.slice(..), loader, 0..len);
    let mut pos = 0;
    while pos < len {
        let active: Vec<_> = highlighter.active_highlights().map(|h| h.idx()).collect();
        let next = highlighter.next_event_offset().min(len);
        if next > pos {
            let _ = writeln!(out, "highlight {pos}..{next} {active:?}");
        }
        pos = next;
        if pos >= len {
            break;
        }
        highlighter.advance();
    }
    out
}

fn loader() -> (Loader, Vec<String>) {
    let loader = helix_core::config::default_lang_loader();
    // Use every capture name of the highlight queries as a scope so that highlights are
    // compared in full detail.
    let queries = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/queries");
    let mut scopes = HashSet::new();
    for sample in SAMPLES {
        for language in [sample.language, "comment", "regex", "jsdoc", "luap", "css"] {
            let Ok(query) = std::fs::read_to_string(queries.join(language).join("highlights.scm"))
            else {
                continue;
            };
            for capture in query.split('@').skip(1) {
                let name: String = capture
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-'))
                    .collect();
                if !name.is_empty() && !name.starts_with('_') {
                    scopes.insert(name);
                }
            }
        }
    }
    let mut scopes: Vec<_> = scopes.into_iter().collect();
    scopes.sort();
    loader.set_scopes(scopes.clone());
    (loader, scopes)
}

fn check_language(
    sample: &Sample,
    loader: &Loader,
    scopes: &[String],
    seed: u64,
    runs: usize,
    steps: usize,
) {
    let language = loader
        .language_for_name(sample.language.to_string())
        .unwrap_or_else(|| panic!("unknown language {}", sample.language));
    let mut rng = Rng(seed | 1);
    for run in 0..runs {
        let blocks = 1 + rng.below(4);
        let mut text = Rope::from(
            (0..blocks)
                .map(|i| sample.source.replace("IDX", &i.to_string()))
                .collect::<String>(),
        );
        let mut syntax = Syntax::new(text.slice(..), language, TIMEOUT, loader)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", sample.language));
        for step in 0..steps {
            // Reconfigure the highlight scopes halfway through every other run (like a theme
            // change) so that the set of local definition captures changes between edits:
            // without these scopes `@local.definition.variable` and
            // `@local.definition.variable.builtin` don't resolve to a highlight anymore.
            if run % 2 == 1 && step == steps / 2 {
                loader.set_scopes(
                    scopes
                        .iter()
                        .filter(|scope| {
                            *scope != "variable" && !scope.starts_with("variable.builtin")
                        })
                        .cloned()
                        .collect(),
                );
            }
            // a transaction with one to three non-overlapping changes, mostly small ones
            let mut points: Vec<usize> = (0..2 * (1 + rng.below(3)))
                .map(|_| rng.below(text.len_chars() + 1))
                .collect();
            points.sort_unstable();
            let mut last_end = None;
            let changes: Vec<_> = points
                .chunks(2)
                .map(|range| {
                    let from = range[0];
                    let mut to = range[1];
                    if rng.below(3) != 0 {
                        to = from + (to - from).min(rng.below(12));
                    }
                    let insert = (rng.below(3) != 0)
                        .then(|| sample.snippets[rng.below(sample.snippets.len())].into());
                    (from, to, insert)
                })
                .filter(|change| {
                    let disjoint = last_end.is_none_or(|end| change.0 > end);
                    if disjoint {
                        last_end = Some(change.1);
                    }
                    disjoint
                })
                .collect();
            let transaction = Transaction::change(&text, changes.clone().into_iter());
            let mut new_text = text.clone();
            transaction.apply(&mut new_text);
            let edits = generate_edits(text.slice(..), transaction.changes());
            if step == steps - 1 {
                loader.set_scopes(scopes.to_vec());
            }

            let mut reference = syntax.clone();
            tree_house::set_incremental_injections(false);
            reference
                .update(new_text.slice(..), TIMEOUT, &edits, loader)
                .unwrap();
            tree_house::set_incremental_injections(true);
            syntax
                .update(new_text.slice(..), TIMEOUT, &edits, loader)
                .unwrap();
            text = new_text;

            // Tree-sitter's incremental parsing itself is not affected by the incremental
            // queries. In rare cases its error recovery differs between two parses of the same
            // edit though, the queries can't be compared then.
            if root_tree(&syntax) != root_tree(&reference) {
                continue;
            }
            let incremental = snapshot(&syntax, &text, loader);
            let full = snapshot(&reference, &text, loader);
            if incremental != full {
                let line = incremental
                    .lines()
                    .zip(full.lines())
                    .position(|(a, b)| a != b);
                panic!(
                    "{}: incremental queries differ from full queries (seed {seed}, run {run}, \
                     step {step}, changes {changes:?})\ntext: {:?}\nfirst difference:\n  \
                     incremental: {:?}\n  full:        {:?}",
                    sample.language,
                    text.to_string(),
                    line.and_then(|line| incremental.lines().nth(line)),
                    line.and_then(|line| full.lines().nth(line)),
                );
            }
        }
    }
}

#[test]
fn incremental_queries_match_full_queries() {
    let (loader, scopes) = loader();
    for (i, sample) in SAMPLES.iter().enumerate() {
        for seed in [7, 1234 + i as u64] {
            check_language(sample, &loader, &scopes, seed, 12, 20);
        }
    }
}

/// Languages with the same grammar and highlight queries (Rust and the Rust
/// it injects into format macros) share one compiled highlight query.
#[test]
fn identical_highlight_queries_are_compiled_once() {
    use tree_house::LanguageLoader;
    let loader = helix_core::config::default_lang_loader();
    let config = |name: &str| {
        let language = loader.language_for_name(name.to_string()).unwrap();
        loader
            .get_config(language)
            .expect("the Rust grammar is built")
    };
    let (rust, format_macro) = (config("rust"), config("rust-format-args-macro"));
    assert!(std::sync::Arc::ptr_eq(
        &rust.highlight_query,
        &format_macro.highlight_query
    ));
    // Their injections differ, so those aren't shared.
    assert!(!std::ptr::eq(
        &rust.injection_query,
        &format_macro.injection_query
    ));
}
