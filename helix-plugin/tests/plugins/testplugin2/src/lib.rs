use std::cell::Cell;

use helix_plugin_api::{
    editor, register_plugin, requests, CommandInfo, DocumentId, Edit, EventKind,
    InteractiveOptions, Item, Mode, Picker, Plugin, Registration, SpawnOptions,
};

thread_local! {
    static EVENT_UI: Cell<bool> = const { Cell::new(false) };
}

struct TestPlugin;
impl Plugin for TestPlugin {
    fn init(config: String) -> Result<Registration, String> {
        EVENT_UI.set(config.contains("event-ui"));
        editor::set_status(&format!("0.2 init:{config}"));
        Ok(Registration {
            commands: [
                "inspect",
                "arm-process",
                "arm-picker",
                "spawn",
                "interactive",
                "picker",
                "push-items",
                "kill",
            ]
            .into_iter()
            .map(|name| CommandInfo {
                name: name.into(),
                doc: format!("Test {name}"),
            })
            .collect(),
            events: vec![EventKind::DocumentOpen, EventKind::ModeSwitch],
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "inspect" => {
                let doc = editor::current_document().ok_or("no document")?;
                let text = editor::text(doc)?;
                let ranges = editor::selections(doc)?;
                editor::set_selections(doc, &ranges)?;
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: 0,
                        to: 0,
                        text: "v2:".into(),
                    }],
                )?;
                editor::set_status(&format!("{text}:{:?}", editor::current_mode()));
            }
            // Host tests reserve ids directly to exercise completion delivery
            // before the actual asynchronous operations land in M1/M2.
            "arm-process" => {
                let id = request_id(&args)?;
                requests::register_process(id, move |result| {
                    if let Ok(output) = &result {
                        match output.stdout.as_str() {
                            "read" => {
                                let doc = editor::current_document().ok_or("no document")?;
                                editor::set_status(&editor::text(doc)?);
                                return Ok(());
                            }
                            "command" => return editor::run_command("goto_line_end"),
                            "fail" => return Err("callback failed on purpose".into()),
                            "spin" => loop {
                                std::hint::spin_loop();
                            },
                            _ => {}
                        }
                    }
                    editor::set_status(&format!("process:{id}:{result:?}"));
                    Ok(())
                });
            }
            "arm-picker" => {
                let id = request_id(&args)?;
                requests::register_picker(id, move |result| {
                    editor::set_status(&format!("picker:{id}:{result:?}"));
                    Ok(())
                });
            }
            "spawn" => {
                editor::spawn(&args, SpawnOptions::default(), |output| {
                    let output = output?;
                    editor::set_status(&format!(
                        "exit:{:?};out:{};err:{}",
                        output.exit_code, output.stdout, output.stderr
                    ));
                    Ok(())
                })?;
            }
            "interactive" => {
                editor::run_interactive(
                    &args,
                    InteractiveOptions {
                        chooser_file: true,
                        ..Default::default()
                    },
                    |output| {
                        editor::set_status(&output?.stdout);
                        Ok(())
                    },
                )?;
            }
            "picker" => {
                editor::pick(
                    Picker::new("Test", args.into_iter().map(Item::text).collect()),
                    |choice| {
                        editor::set_status(&format!(
                            "choice:{:?}:{:?}",
                            choice.indices, choice.action
                        ));
                        Ok(())
                    },
                )?;
            }
            "push-items" => {
                editor::push_items(request_id(&args)?, vec![Item::text("new item")])?;
            }
            "kill" => editor::kill(request_id(&args)?),
            _ => return Err(format!("unknown command {name}")),
        }
        Ok(())
    }

    fn on_document_open(doc: DocumentId) {
        if EVENT_UI.get() {
            let interactive =
                editor::run_interactive(&["true"], InteractiveOptions::default(), |_| Ok(()));
            let picker = editor::pick(Picker::new("Event", vec![]), |_| Ok(()));
            editor::set_status(&interactive.unwrap_err());
            editor::set_status(&picker.unwrap_err());
            editor::set_status(&editor::run_command("goto_line_end").unwrap_err());
        } else {
            editor::set_status(&format!("open:{doc}"));
        }
    }
    fn on_mode_switch(old: Mode, new: Mode) {
        editor::set_status(&format!("{old:?}->{new:?}"));
    }
}

fn request_id(args: &[String]) -> Result<u32, String> {
    args.first()
        .ok_or_else(|| "missing request id".into())
        .and_then(|id| id.parse().map_err(|err| format!("invalid id: {err}")))
}

register_plugin!(TestPlugin);
