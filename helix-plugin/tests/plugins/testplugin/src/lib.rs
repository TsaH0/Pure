//! Plugin used by Helix's tests. Every command exercises one part of the API
//! or one way a plugin can misbehave.

use std::sync::Mutex;

use helix_plugin_api::{
    editor, register_plugin, text, CommandInfo, Edit, EventKind, Mode, Plugin, Range, Registration,
};

static CONFIG: Mutex<String> = Mutex::new(String::new());
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn config_has(flag: &str) -> bool {
    CONFIG.lock().unwrap().contains(flag)
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Insert => "insert",
        Mode::Select => "select",
    }
}

fn current() -> Result<u32, String> {
    editor::current_document().ok_or_else(|| "no document".to_string())
}

const COMMANDS: &[(&str, &str)] = &[
    ("status", "Show the arguments in the status line"),
    ("set-error", "Show the arguments as an error"),
    ("fail", "Return an error"),
    ("info", "Show what the plugin can see"),
    ("insert", "Insert the arguments at the primary cursor"),
    (
        "two-edits",
        "Insert 'a' then 'b' at the start, as two edits",
    ),
    ("replace-all", "Replace the document with the arguments"),
    ("select-all", "Select the whole document"),
    ("open", "Open the file given as argument"),
    ("run", "Run the editor command given as arguments"),
    ("bad-edit", "Make an edit past the end of the document"),
    ("overlap", "Make overlapping edits"),
    ("read", "Read the current document's text"),
    ("events", "Show the events received so far"),
    ("loop", "Loop forever"),
    ("panic", "Panic"),
    ("alloc", "Allocate memory until it runs out"),
];

struct TestPlugin;

impl Plugin for TestPlugin {
    fn init(config: String) -> Result<Registration, String> {
        *CONFIG.lock().unwrap() = config;
        if config_has("fail-init") {
            return Err("init failed on purpose".into());
        }
        if config_has("init-status") {
            editor::set_status("testplugin loaded");
        }
        let mut commands: Vec<CommandInfo> = COMMANDS
            .iter()
            .map(|(name, doc)| CommandInfo {
                name: name.to_string(),
                doc: doc.to_string(),
            })
            .collect();
        if config_has("bad-command-name") {
            commands.push(CommandInfo {
                name: "Bad Name".into(),
                doc: String::new(),
            });
        }
        let events = if config_has("no-events") {
            vec![]
        } else {
            vec![
                EventKind::DocumentOpen,
                EventKind::DocumentChange,
                EventKind::ModeSwitch,
            ]
        };
        Ok(Registration { commands, events })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        let joined = args.join(" ");
        match name {
            "status" => editor::set_status(&joined),
            "set-error" => editor::set_error(&joined),
            "fail" => return Err(format!("failed: {joined}")),
            "info" => {
                let doc = current()?;
                let path = editor::document_path(doc)?.unwrap_or_else(|| "none".into());
                let lang = editor::document_language(doc)?.unwrap_or_else(|| "none".into());
                let sel = editor::selections(doc)?;
                let sel = sel
                    .iter()
                    .map(|r| format!("{}-{}", r.anchor, r.head))
                    .collect::<Vec<_>>()
                    .join(",");
                let len = text::char_len(&editor::text(doc)?);
                editor::set_status(&format!(
                    "path={path} lang={lang} mode={} docs={} sel={sel} len={len}",
                    mode_name(editor::current_mode()),
                    editor::documents().len()
                ));
            }
            "insert" => {
                let doc = current()?;
                let head = editor::selections(doc)?[0].head;
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: head,
                        to: head,
                        text: joined,
                    }],
                )?;
            }
            "two-edits" => {
                let doc = current()?;
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: 0,
                        to: 0,
                        text: "a".into(),
                    }],
                )?;
                // Offsets refer to the text after the first edit.
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: 0,
                        to: 0,
                        text: "b".into(),
                    }],
                )?;
                let start = editor::text_range(doc, 0, 2)?;
                editor::set_status(&format!("start={start}"));
            }
            "replace-all" => {
                let doc = current()?;
                let len = text::char_len(&editor::text(doc)?);
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: 0,
                        to: len,
                        text: joined,
                    }],
                )?;
            }
            "select-all" => {
                let doc = current()?;
                let len = text::char_len(&editor::text(doc)?);
                editor::set_selections(
                    doc,
                    &[Range {
                        anchor: 0,
                        head: len,
                    }],
                )?;
            }
            "open" => editor::open_file(&joined)?,
            "run" => editor::run_command(&joined)?,
            "bad-edit" => {
                let doc = current()?;
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: 0,
                        to: 1_000_000,
                        text: String::new(),
                    }],
                )?;
            }
            "overlap" => {
                let doc = current()?;
                editor::apply_edits(
                    doc,
                    &[
                        Edit {
                            from: 0,
                            to: 2,
                            text: "x".into(),
                        },
                        Edit {
                            from: 1,
                            to: 3,
                            text: "y".into(),
                        },
                    ],
                )?;
            }
            "read" => {
                let doc = current()?;
                let text = editor::text(doc)?;
                editor::set_status(&format!("read {} chars", text::char_len(&text)));
            }
            "events" => {
                editor::set_status(&format!("events: {}", EVENTS.lock().unwrap().join(",")))
            }
            "loop" => {
                let mut i: u64 = 0;
                loop {
                    i = std::hint::black_box(i.wrapping_add(1));
                }
            }
            "panic" => panic!("panic on purpose"),
            "alloc" => {
                let mut chunks: Vec<Vec<u8>> = Vec::new();
                loop {
                    chunks.push(std::hint::black_box(vec![1u8; 16 << 20]));
                }
            }
            _ => return Err(format!("unknown command {name}")),
        }
        Ok(())
    }

    fn on_document_open(doc: u32) {
        EVENTS.lock().unwrap().push(format!("open:{doc}"));
        if config_has("run-on-open") {
            // Not allowed outside of commands.
            if let Err(err) = editor::run_command(":write") {
                EVENTS.lock().unwrap().push(format!("run-error:{err}"));
            }
        }
        if config_has("status-on-open") {
            let path = editor::document_path(doc)
                .ok()
                .flatten()
                .unwrap_or_default();
            editor::set_status(&format!("opened {path}"));
        }
    }

    fn on_document_change(doc: u32) {
        EVENTS.lock().unwrap().push(format!("change:{doc}"));
        if config_has("edit-on-change") {
            // Edits from an event handler must not trigger the handler again.
            if let Ok(text) = editor::text(doc) {
                let end = text::char_len(&text);
                let _ = editor::apply_edits(
                    doc,
                    &[Edit {
                        from: end,
                        to: end,
                        text: "!".into(),
                    }],
                );
            }
        }
        if config_has("loop-on-change") {
            let mut i: u64 = 0;
            loop {
                i = std::hint::black_box(i.wrapping_add(1));
            }
        }
    }

    fn on_mode_switch(old: Mode, new: Mode) {
        EVENTS
            .lock()
            .unwrap()
            .push(format!("mode:{}>{}", mode_name(old), mode_name(new)));
    }
}

register_plugin!(TestPlugin);
