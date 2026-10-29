//! Example plugin: script the editor in [Rhai](https://rhai.rs), the way
//! Neovim users write a few lines of Lua in `init.lua`. The script runs at
//! startup and can define commands, bind keys and react to events:
//!
//! ```rhai
//! // :script/upper, and space u for it
//! define("upper", "Uppercase the selections", "upper");
//! bind("normal", "space u", "upper");
//! fn upper() { replace_selections(selected().map(|s| s.to_upper())); }
//!
//! // Say which function the cursor is in, after each save.
//! on("save", "saved");
//! fn saved() { status(`saved ${path()}`); }
//! ```
//!
//! `:script/eval <code>` runs a line of Rhai and shows its value, like
//! `:lua`. The script comes from `source` (inline) or `file` in
//! `plugins.toml`, or `init.rhai` in the config directory.

use std::cell::RefCell;
use std::collections::HashMap;

use helix_plugin_api::{
    commands, editor,
    keymaps::{self, binding},
    register_plugin, syntax, CommandInfo, DocumentId, Edit, EventKind, KeyOutcome, Mode, Plugin,
    Range, Registration,
};
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Scope, AST};

/// Work one call of the script may do before it is stopped (an endless
/// loop shouldn't get the plugin disabled).
const MAX_OPERATIONS: u64 = 20_000_000;

#[derive(Default)]
struct Script {
    ast: AST,
    scope: Scope<'static>,
    /// `define`d commands: name -> (function, description).
    commands: Vec<(String, String, String)>,
    /// `on` handlers by event name.
    handlers: HashMap<String, Vec<String>>,
}

thread_local! {
    static ENGINE: Engine = engine();
    static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
    /// What `define` and `on` ask for while the script first runs.
    static DEFINED: RefCell<Vec<(String, String, String)>> = const { RefCell::new(Vec::new()) };
    static HANDLERS: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
}

/// What script functions return: errors become Rhai errors.
type Ret<T> = Result<T, Box<EvalAltResult>>;

fn fail<T>(message: impl Into<String>) -> Ret<T> {
    Err(message.into().into())
}

fn current() -> Ret<DocumentId> {
    editor::current_document().ok_or_else(|| "no document".into())
}

fn host<T>(result: Result<T, String>) -> Ret<T> {
    result.map_err(Into::into)
}

/// Selection `i` as `(from, to)`.
fn span(range: &Range) -> (u32, u32) {
    (range.anchor.min(range.head), range.anchor.max(range.head))
}

fn selected() -> Ret<Array> {
    let doc = current()?;
    let text: Vec<char> = host(editor::text(doc))?.chars().collect();
    Ok(host(editor::selections(doc))?
        .iter()
        .map(|range| {
            let (from, to) = span(range);
            let to = (to as usize).max(from as usize + 1).min(text.len());
            Dynamic::from(text[from as usize..to].iter().collect::<String>())
        })
        .collect())
}

fn replace_selections(texts: Array) -> Ret<()> {
    let doc = current()?;
    let selections = host(editor::selections(doc))?;
    if texts.len() != selections.len() {
        return fail(format!(
            "{} texts for {} selections",
            texts.len(),
            selections.len()
        ));
    }
    let len = host(editor::text(doc))?.chars().count() as u32;
    let edits: Vec<Edit> = selections
        .iter()
        .zip(texts)
        .map(|(range, text)| {
            let (from, to) = span(range);
            Edit {
                from,
                // A cursor covers the character under it.
                to: to.max(from + 1).min(len),
                text: text.to_string(),
            }
        })
        .collect();
    host(editor::apply_edits(doc, &edits))
}

fn insert(text: &str) -> Ret<()> {
    let doc = current()?;
    let edits: Vec<Edit> = host(editor::selections(doc))?
        .iter()
        .map(|range| Edit {
            from: range.head,
            to: range.head,
            text: text.to_string(),
        })
        .collect();
    host(editor::apply_edits(doc, &edits))
}

fn selections() -> Ret<Array> {
    Ok(host(editor::selections(current()?))?
        .iter()
        .map(|range| {
            let mut map = Map::new();
            map.insert("anchor".into(), (range.anchor as i64).into());
            map.insert("head".into(), (range.head as i64).into());
            map.into()
        })
        .collect())
}

fn set_selections(ranges: Array) -> Ret<()> {
    let ranges: Vec<Range> = ranges
        .into_iter()
        .map(|range| {
            let map = range
                .try_cast::<Map>()
                .ok_or("a selection is #{anchor, head}")?;
            let get = |key: &str| {
                map.get(key)
                    .and_then(|value| value.as_int().ok())
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or(format!("a selection needs a non-negative {key}"))
            };
            Ok(Range {
                anchor: get("anchor")?,
                head: get("head")?,
            })
        })
        .collect::<Result<_, String>>()?;
    host(editor::set_selections(current()?, &ranges))
}

/// The captures of a tree-sitter query over the current document.
fn query(source: &str) -> Ret<Array> {
    let doc = current()?;
    let text: Vec<char> = host(editor::text(doc))?.chars().collect();
    Ok(host(syntax::query(doc, source, None))?
        .into_iter()
        .flat_map(|found| found.captures)
        .map(|capture| {
            let node = capture.node;
            let mut map = Map::new();
            map.insert("name".into(), capture.name.into());
            map.insert("kind".into(), node.kind.into());
            map.insert("start".into(), (node.start as i64).into());
            map.insert("end".into(), (node.end as i64).into());
            let (start, end) = (node.start as usize, (node.end as usize).min(text.len()));
            map.insert(
                "text".into(),
                text[start.min(end)..end].iter().collect::<String>().into(),
            );
            map.into()
        })
        .collect())
}

/// Run editor command `name` (`:w` is "write") after this call; a failure
/// is shown.
fn command(name: &str, args: Array) -> Ret<()> {
    let args: Vec<String> = args.into_iter().map(|arg| arg.to_string()).collect();
    host(commands::execute(name, &args, None, |result| result).map(|_| ()))
}

fn mode_of(name: &str) -> Ret<Mode> {
    match name {
        "normal" => Ok(Mode::Normal),
        "insert" => Ok(Mode::Insert),
        "select" => Ok(Mode::Select),
        _ => fail(format!("unknown mode '{name}': normal, insert or select")),
    }
}

fn bind(mode: &str, keys: &str, function: &str) -> Ret<()> {
    let function = function.to_string();
    let name = format!("{}:{keys}", function);
    host(keymaps::bind(
        binding(mode_of(mode)?, keys, &name, &function),
        move |_keys, _count| {
            let _ = call(&function, ())?;
            Ok(KeyOutcome::Handled)
        },
    ))
}

const EVENTS: &[(&str, EventKind)] = &[
    ("open", EventKind::DocumentOpen),
    ("change", EventKind::DocumentChange),
    ("save", EventKind::DocumentSave),
    ("close", EventKind::DocumentClose),
    ("mode", EventKind::ModeSwitch),
    ("idle", EventKind::EditorIdle),
];

fn on(event: &str, function: &str) -> Ret<()> {
    if !EVENTS.iter().any(|(name, _)| *name == event) {
        let names: Vec<&str> = EVENTS.iter().map(|(name, _)| *name).collect();
        return fail(format!("unknown event '{event}': {}", names.join(", ")));
    }
    HANDLERS.with_borrow_mut(|handlers| {
        handlers
            .entry(event.to_string())
            .or_default()
            .push(function.to_string())
    });
    Ok(())
}

fn define(name: &str, description: &str, function: &str) -> Ret<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return fail(format!(
            "invalid command name '{name}': use a-z, 0-9 and '-'"
        ));
    }
    DEFINED.with_borrow_mut(|defined| {
        defined.push((name.into(), function.into(), description.into()))
    });
    Ok(())
}

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(MAX_OPERATIONS);
    engine.on_print(|message| editor::set_status(message));
    engine.on_debug(|message, _, _| editor::log(message));
    engine
        .register_fn("status", |message: &str| editor::set_status(message))
        .register_fn("error", |message: &str| editor::set_error(message))
        .register_fn("log", |message: &str| editor::log(message))
        .register_fn("text", || -> Ret<String> { host(editor::text(current()?)) })
        .register_fn("path", || -> Ret<String> {
            Ok(host(editor::document_path(current()?))?.unwrap_or_default())
        })
        .register_fn("language", || -> Ret<String> {
            Ok(host(editor::document_language(current()?))?.unwrap_or_default())
        })
        .register_fn("mode", || match editor::current_mode() {
            Mode::Normal => "normal",
            Mode::Insert => "insert",
            Mode::Select => "select",
        })
        .register_fn("selections", selections)
        .register_fn("set_selections", set_selections)
        .register_fn("selected", selected)
        .register_fn("replace_selections", replace_selections)
        .register_fn("insert", insert)
        .register_fn("query", query)
        .register_fn("command", command)
        .register_fn("command", |name: &str| command(name, Array::new()))
        .register_fn("bind", bind)
        .register_fn("on", on)
        .register_fn("define", define);
    engine
}

/// Call script function `name`.
fn call(name: &str, args: impl rhai::FuncArgs) -> Result<Dynamic, String> {
    ENGINE.with(|engine| {
        SCRIPT.with_borrow_mut(|script| {
            let Script { ast, scope, .. } = script;
            engine
                .call_fn::<Dynamic>(scope, ast, name, args)
                .map_err(|err| format!("script: {name}: {err}"))
        })
    })
}

fn handle(event: &str) {
    let handlers = SCRIPT.with_borrow(|script| script.handlers.get(event).cloned());
    for function in handlers.unwrap_or_default() {
        if let Err(err) = call(&function, ()) {
            editor::set_error(&err);
        }
    }
}

/// The script: `source`, else `file`, else `init.rhai` in the config
/// directory (silently skipped if missing).
fn script_source(config: &serde_json::Value) -> Result<String, String> {
    if let Some(source) = config["source"].as_str().filter(|s| !s.is_empty()) {
        return Ok(source.to_string());
    }
    match config["file"].as_str().filter(|s| !s.is_empty()) {
        Some(file) => std::fs::read_to_string(file).map_err(|err| {
            format!("script: can't read {file}: {err} (allow-files and allow-dirs?)")
        }),
        None => {
            let default = format!("{}/init.rhai", editor::config_dir());
            Ok(std::fs::read_to_string(default).unwrap_or_default())
        }
    }
}

struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn init(config: String) -> Result<Registration, String> {
        let config: serde_json::Value = serde_json::from_str(&config).unwrap_or_default();
        let source = script_source(&config)?;
        let ast = ENGINE
            .with(|engine| engine.compile(&source))
            .map_err(|err| format!("script: {err}"))?;
        let mut scope = Scope::new();
        // Run the top level: definitions, bindings, handlers.
        ENGINE
            .with(|engine| engine.run_ast_with_scope(&mut scope, &ast))
            .map_err(|err| format!("script: {err}"))?;
        let commands = DEFINED.take();
        let handlers = HANDLERS.take();
        let events = EVENTS
            .iter()
            .filter(|(name, _)| handlers.contains_key(*name))
            .map(|(_, kind)| *kind)
            .collect();
        let mut registration = vec![CommandInfo {
            name: "eval".into(),
            doc: "Run a line of Rhai and show its value".into(),
        }];
        registration.extend(commands.iter().map(|(name, _, doc)| CommandInfo {
            name: name.clone(),
            doc: doc.clone(),
        }));
        SCRIPT.set(Script {
            ast,
            scope,
            commands,
            handlers,
        });
        Ok(Registration {
            commands: registration,
            events,
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        if name == "eval" {
            let code = args.join(" ");
            let value = ENGINE.with(|engine| {
                SCRIPT.with_borrow_mut(|script| {
                    // The script's functions are available too.
                    let ast = script.ast.merge(&engine.compile(&code)?);
                    engine.eval_ast_with_scope::<Dynamic>(&mut script.scope, &ast)
                })
            });
            return match value {
                Ok(value) if value.is_unit() => Ok(()),
                Ok(value) => {
                    editor::set_status(&value.to_string());
                    Ok(())
                }
                Err(err) => Err(format!("script: {err}")),
            };
        }
        let function = SCRIPT.with_borrow(|script| {
            script
                .commands
                .iter()
                .find(|(command, ..)| command == name)
                .map(|(_, function, _)| function.clone())
        });
        let function = function.ok_or_else(|| format!("unknown command {name}"))?;
        call(&function, ()).map(|_| ())
    }

    fn on_document_open(_doc: DocumentId) {
        handle("open");
    }
    fn on_document_change(_doc: DocumentId) {
        handle("change");
    }
    fn on_document_save(_doc: DocumentId) {
        handle("save");
    }
    fn on_document_close(_doc: DocumentId) {
        handle("close");
    }
    fn on_mode_switch(_old: Mode, _new: Mode) {
        handle("mode");
    }
    fn on_editor_idle() {
        handle("idle");
    }
}

register_plugin!(ScriptPlugin);
