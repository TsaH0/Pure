//! A direct-bindings fixture for the API 0.3 world, independent of the SDK.
wit_bindgen::generate!({ path: "../../../wit/v0.4", world: "helix-plugin" });
use helix::plugin::{host, types};
use std::cell::{Cell, RefCell};
thread_local! {
    static POPUPS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static CLOSED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static POPUP_ON_OPEN: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static PICKER: Cell<Option<u32>> = const { Cell::new(None) };
    /// Counted in the instance that runs: the background one has its own.
    static RUNS: Cell<u32> = const { Cell::new(0) };
}
fn item(text: &str) -> helix::plugin::ui::Item {
    helix::plugin::ui::Item {
        columns: vec![text.into()],
        data: text.into(),
        preview: None,
    }
}
fn record(event: String) {
    host::log(&format!("event:{event}"));
    EVENTS.with_borrow_mut(|events| events.push(event));
}
struct TestPlugin;
impl exports::helix::plugin::plugin::Guest for TestPlugin {
    fn init(config: String) -> Result<types::Registration, String> {
        POPUP_ON_OPEN.set(config.contains("popup-on-open"));
        Ok(types::Registration {
            commands: [
                "inspect",
                "open-at",
                "popup",
                "close-popup",
                "popup-close-now",
                "two-popups",
                "popup-events",
                "events",
                "stream",
                "stream-fail",
                "spawn-kill",
                "spawn-reload",
                "spawn-disable",
                "interactive-reload",
                "interactive-disable",
                "interactive-twice",
                "background",
                "pause",
                "loop",
                "output-events",
                "kill",
                "live",
                "live-fail",
                "static",
                "items",
                "diagnose",
                "diagnose-bad",
                "timer",
                "timer-cancel",
                "cancel-timer",
                "scratch",
                "buffer",
                "registers",
                "read-clipboard",
                "files",
            ]
            .into_iter()
            .map(|name| types::CommandInfo {
                name: name.into(),
                doc: name.into(),
            })
            .collect(),
            events: {
                let mut events = vec![types::EventKind::DocumentOpen];
                if config.contains("completion") {
                    events.push(types::EventKind::Completion);
                }
                if config.contains("extended-events") {
                    events.extend([
                        types::EventKind::PostCommand,
                        types::EventKind::FocusGained,
                        types::EventKind::FocusLost,
                        types::EventKind::EditorIdle,
                    ]);
                }
                events
            },
        })
    }
    fn run_command(name: String, args: Vec<String>) -> Result<(), String> {
        match name.as_str() {
            "loop" => loop {
                std::hint::spin_loop();
            },
            "pause" => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                Ok(())
            }
            // Two terminal programs from one call: only one can have the
            // terminal; the first is answered with an error.
            "interactive-twice" => {
                let options = helix::plugin::process::InteractiveOptions {
                    cwd: None,
                    chooser_file: true,
                    wait_for_key: false,
                };
                let first = helix::plugin::process::run_interactive(&args, &options)?;
                let second = helix::plugin::process::run_interactive(&args, &options)?;
                host::log(&format!("twice:{first}:{second}"));
                Ok(())
            }
            // background <task> <input>: the result arrives in a callback.
            "background" => {
                let (task, input) = (&args[0], args.get(1).map_or("", String::as_str));
                RUNS.set(RUNS.get() + 1);
                let id = helix::plugin::worker::run_background(task, input)?;
                host::log(&format!("background:{id}"));
                Ok(())
            }
            "interactive-reload" | "interactive-disable" => {
                helix::plugin::process::run_interactive(
                    &args,
                    &helix::plugin::process::InteractiveOptions {
                        cwd: None,
                        chooser_file: true,
                        wait_for_key: false,
                    },
                )?;
                host::run_command(if name == "interactive-reload" {
                    ":plugin-reload"
                } else {
                    ":plugin-run testplugin3/loop"
                })
            }
            "live" | "live-fail" | "static" => {
                let id = helix::plugin::ui::show_picker(&helix::plugin::ui::Picker {
                    title: "Generations".into(),
                    columns: vec!["Name".into()],
                    items: vec![item("initial")],
                    initial: None,
                    live: name != "static",
                })?;
                PICKER.set(Some(id));
                if name == "live-fail" {
                    return Err("failed after picker".into());
                }
                host::set_status(&format!("picker:{id}"));
                Ok(())
            }
            "items" => {
                let id = PICKER.get().ok_or("no picker")?;
                let generation = match args.first().map(String::as_str) {
                    Some("-") => None,
                    Some(generation) => Some(
                        generation
                            .parse::<u64>()
                            .map_err(|error| error.to_string())?,
                    ),
                    None => return Err("missing generation".into()),
                };
                let items = args
                    .iter()
                    .skip(2)
                    .map(|text| item(text))
                    .collect::<Vec<_>>();
                match args.get(1).map(String::as_str) {
                    Some("push") => helix::plugin::ui::push_items(id, generation, &items),
                    Some("set") => helix::plugin::ui::set_items(id, generation, &items),
                    _ => Err("missing push/set".into()),
                }
            }
            "stream" | "stream-fail" | "spawn-kill" | "spawn-reload" | "spawn-disable" => {
                let id = helix::plugin::process::spawn(
                    &args,
                    &helix::plugin::process::SpawnOptions {
                        cwd: None,
                        stdin: None,
                        timeout_ms: Some(5000),
                        stream: true,
                    },
                )?;
                if name == "stream-fail" {
                    return Err("failed after spawn".into());
                }
                if name == "spawn-kill" {
                    helix::plugin::process::kill(id);
                }
                if name == "spawn-reload" {
                    host::run_command(":plugin-run testplugin3/pause")?;
                    host::run_command(":plugin-reload")?;
                    return Ok(());
                }
                if name == "spawn-disable" {
                    return host::run_command(":plugin-run testplugin3/loop");
                }
                host::set_status(&format!("stream-id:{id}"));
                Ok(())
            }
            "kill" => {
                let id = args
                    .first()
                    .ok_or("missing id")?
                    .parse::<u32>()
                    .map_err(|error| error.to_string())?;
                helix::plugin::process::kill(id);
                Ok(())
            }
            "output-events" => {
                OUTPUT.with_borrow(|output| host::set_status(&format!("output:{output:?}")));
                Ok(())
            }
            "events" => {
                EVENTS.with_borrow(|events| host::set_status(&events.join(",")));
                Ok(())
            }
            "popup" | "popup-close-now" => {
                let id = helix::plugin::ui::show_popup(&helix::plugin::ui::Popup {
                    markdown: args.iter().skip(1).cloned().collect::<Vec<_>>().join(" "),
                    centered: args.first().is_some_and(|arg| arg == "centered"),
                })?;
                POPUPS.with_borrow_mut(|popups| popups.push(id));
                host::set_status(&format!("popup:{id}"));
                if name == "popup-close-now" {
                    helix::plugin::ui::close_popup(id);
                }
                Ok(())
            }
            "two-popups" => {
                let mut ids = Vec::new();
                for text in ["first", "second"] {
                    let id = helix::plugin::ui::show_popup(&helix::plugin::ui::Popup {
                        markdown: text.into(),
                        centered: false,
                    })?;
                    POPUPS.with_borrow_mut(|popups| popups.push(id));
                    ids.push(id);
                }
                host::set_status(&format!("popups:{},{}", ids[0], ids[1]));
                Ok(())
            }
            "close-popup" => {
                let id = args
                    .first()
                    .map(|arg| arg.parse::<u32>().map_err(|error| error.to_string()))
                    .transpose()?
                    .or_else(|| POPUPS.with_borrow(|popups| popups.last().copied()))
                    .ok_or("no popup")?;
                helix::plugin::ui::close_popup(id);
                Ok(())
            }
            "popup-events" => {
                CLOSED.with_borrow(|closed| host::set_status(&format!("closed:{closed:?}")));
                Ok(())
            }
            "inspect" => {
                let doc = host::current_document().ok_or("no document")?;
                let text = host::text(doc)?;
                host::apply_edits(
                    doc,
                    &[types::Edit {
                        from: 0,
                        to: 0,
                        text: "v3:".into(),
                    }],
                )?;
                host::set_status(&format!("0.3:{text}"));
                Ok(())
            }
            "open-at" => {
                let path = args.first().ok_or("missing path")?;
                let action = match args.get(1).map(String::as_str).unwrap_or("replace") {
                    "replace" => types::OpenAction::Replace,
                    "hsplit" => types::OpenAction::Hsplit,
                    "vsplit" => types::OpenAction::Vsplit,
                    _ => return Err("invalid action".into()),
                };
                let number = |index| {
                    args.get(index)
                        .map(|value: &String| {
                            value.parse::<u32>().map_err(|error| error.to_string())
                        })
                        .transpose()
                };
                host::open_file_at(path, action, number(2)?, number(3)?)
            }
            "diagnose" | "diagnose-bad" => {
                use helix::plugin::diagnostics::{self, Diagnostic, Severity};
                let doc = host::current_document().ok_or("no document")?;
                let to = if name == "diagnose" { 1 } else { 1_000_000 };
                diagnostics::set_diagnostics(
                    doc,
                    &[Diagnostic {
                        from: 0,
                        to,
                        severity: Severity::Error,
                        message: "bad".into(),
                        code: Some("T1".into()),
                        source: None,
                    }],
                )
            }
            "timer" => {
                let delay = args.first().and_then(|d| d.parse().ok()).unwrap_or(1);
                let id = helix::plugin::timers::set_timer(delay)?;
                host::log(&format!("timer-set:{id}"));
                Ok(())
            }
            "timer-cancel" => {
                let id = helix::plugin::timers::set_timer(1)?;
                helix::plugin::timers::cancel_timer(id);
                host::log(&format!("timer-cancelled:{id}"));
                Ok(())
            }
            "cancel-timer" => {
                let id = args.first().and_then(|id| id.parse().ok()).unwrap_or(0);
                helix::plugin::timers::cancel_timer(id);
                Ok(())
            }
            "scratch" => host::open_scratch("scratch text\n", Some("markdown"), types::OpenAction::Replace),
            "buffer" => host::open_buffer("test://buffer", "one\n", None, types::OpenAction::Replace),
            "registers" => {
                host::set_register('a', &["x".into(), "y".into()])?;
                let values = host::register('a')?;
                host::set_status(&format!("registers:{}", values.join(",")));
                Ok(())
            }
            "read-clipboard" => host::register('+').map(|_| ()),
            // `files create:<path>` `rename:<from>:<to>` `copy:<from>:<to>`
            // `delete:<path>` `trash:<path>`, in order.
            "files" => {
                use helix::plugin::files::{self, CreateOp, FileOp, MoveOp};
                let ops = args
                    .iter()
                    .map(|arg| {
                        let mut parts = arg.splitn(3, ':');
                        let kind = parts.next().unwrap_or_default();
                        let a = parts.next().unwrap_or_default().to_string();
                        let b = parts.next().unwrap_or_default().to_string();
                        match kind {
                            "create" => FileOp::Create(CreateOp { path: a, directory: false }),
                            "mkdir" => FileOp::Create(CreateOp { path: a, directory: true }),
                            "rename" => FileOp::Rename(MoveOp { from: a, to: b }),
                            "copy" => FileOp::Copy(MoveOp { from: a, to: b }),
                            "delete" => FileOp::Delete(a),
                            _ => FileOp::Trash(a),
                        }
                    })
                    .collect::<Vec<_>>();
                let id = files::apply(&ops)?;
                host::log(&format!("files:{id}"));
                Ok(())
            }
            _ => Err("unknown command".into()),
        }
    }
    fn on_document_open(doc: u32) {
        if let Some(name) = host::buffer_name(doc) {
            host::log(&format!("buffer-open:{name}:{doc}"));
        }
        if POPUP_ON_OPEN.get() {
            host::set_status(
                &helix::plugin::ui::show_popup(&helix::plugin::ui::Popup {
                    markdown: "event".into(),
                    centered: false,
                })
                .unwrap_err(),
            );
        } else {
            host::set_status(&format!("v3-open:{doc}"));
        }
    }
    fn on_document_change(_doc: u32) {}
    fn on_mode_switch(_old: types::Mode, _new: types::Mode) {}
}
impl exports::helix::plugin::callbacks::Guest for TestPlugin {
    fn on_process_done(_id: u32, result: Result<helix::plugin::process::Output, String>) {
        let output = match result {
            Ok(output) => format!(
                "done:{:?}:{}:{}",
                output.exit_code, output.stdout, output.stderr
            ),
            Err(error) => format!("error:{error}"),
        };
        OUTPUT.with_borrow_mut(|events| events.push(output.clone()));
        host::set_status(&output);
    }
    fn on_picker_done(_id: u32, result: helix::plugin::ui::PickerResult) {
        host::set_status(&match result {
            helix::plugin::ui::PickerResult::Accepted((indices, _)) => {
                format!("picked:{indices:?}")
            }
            helix::plugin::ui::PickerResult::Cancelled => "cancelled".into(),
        });
    }
    fn on_picker_query(id: u32, generation: u64, query: String) {
        record(format!("query:{generation}:{query}"));
        helix::plugin::ui::set_items(
            id,
            Some(generation),
            &[item(&format!("{query}:{generation}"))],
        )
        .unwrap();
    }
    fn on_process_output(_id: u32, chunk: String) {
        let text = host::current_document()
            .and_then(|doc| host::text(doc).ok())
            .unwrap_or_default();
        let output = format!("chunk:{chunk}:snapshot:{text}");
        OUTPUT.with_borrow_mut(|events| events.push(output.clone()));
        host::set_status(&output);
    }
    fn on_popup_close(id: u32) {
        CLOSED.with_borrow_mut(|closed| closed.push(id));
        POPUPS.with_borrow_mut(|popups| popups.retain(|popup| *popup != id));
        host::set_status(&format!("popup-closed:{id}"));
    }
    fn on_prompt_done(_id: u32, _answer: Option<String>) {}
    fn on_files_done(id: u32, results: Vec<Result<(), String>>) {
        let results: Vec<String> = results
            .into_iter()
            .map(|result| match result {
                Ok(()) => "ok".to_string(),
                Err(err) => format!("err({err})"),
            })
            .collect();
        host::log(&format!("files-done:{id}:{}", results.join(";")));
        host::set_status(&format!("files-done:{}", results.join(";")));
    }
    fn on_timer(id: u32) {
        host::log(&format!("timer-fired:{id}"));
        host::set_status(&format!("timer-fired:{id}"));
    }
    fn on_background_done(_id: u32, result: Result<String, String>) {
        host::set_status(&match result {
            Ok(value) => format!("background:{value}"),
            Err(error) => format!("background-error:{error}"),
        });
    }
}
impl exports::helix::plugin::events::Guest for TestPlugin {
    fn on_document_save(_doc: u32) {}
    fn on_document_close(_doc: u32) {}
    fn on_selection_change(_doc: u32) {}
    fn on_post_command(command: String) {
        record(format!("command:{command}"));
    }
    fn on_focus_gained() {
        record("focus:gained".into());
    }
    fn on_focus_lost() {
        record("focus:lost".into());
    }
    fn on_editor_idle() {
        record("idle".into());
    }
    fn on_completion(id: u32, _doc: u32, position: u32) {
        use helix::plugin::completion::{complete, CompletionItem};
        let item = |label: &str| CompletionItem {
            label: label.into(),
            insert: None,
            kind: Some("test".into()),
            documentation: None,
            replace_from: None,
        };
        host::log(&format!("completion:{id}:{position}"));
        let _ = complete(id, &[item("alpha"), item("alphabet")]);
    }
    fn on_buffer_write(doc: u32) {
        let name = host::buffer_name(doc).unwrap_or_default();
        let text = host::text(doc).unwrap_or_default();
        host::set_status(&format!("written:{name}:{}", text.trim_end()));
        let _ = host::mark_saved(doc);
    }
}
impl exports::helix::plugin::background::Guest for TestPlugin {
    fn run(task: String, input: String) -> Result<String, String> {
        RUNS.set(RUNS.get() + 1);
        match task.as_str() {
            "square" => {
                let n: u64 = input.parse().map_err(|_| "not a number")?;
                Ok((n * n).to_string())
            }
            // This instance's own counter, and what it can see of the editor.
            "isolation" => Ok(format!(
                "runs:{};document:{:?};text:{:?}",
                RUNS.get(),
                host::current_document(),
                host::current_document().map(host::text)
            )),
            "spin" => loop {
                std::hint::spin_loop();
            },
            _ => Err(format!("unknown task {task}")),
        }
    }
}

export!(TestPlugin);
