//! Optional SDK 0.3 trial plugin. Installed only when the user copies it.
use std::cell::Cell;

use helix_plugin_api::{
    editor, register_plugin, CommandInfo, EventKind, Item, OpenAction, Picker, Plugin,
    PopupOptions, Registration, SpawnOptions,
};

thread_local! {
    static PICKER: Cell<Option<u32>> = const { Cell::new(None) };
}

struct Demo;

impl Plugin for Demo {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: [
                "popup", "close", "live", "update", "push", "rollback", "stream", "open-at",
            ]
            .into_iter()
            .map(|name| CommandInfo {
                name: name.into(),
                doc: format!("Try API 0.3 {name}"),
            })
            .collect(),
            events: vec![
                EventKind::PostCommand,
                EventKind::FocusGained,
                EventKind::FocusLost,
                EventKind::EditorIdle,
            ],
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "popup" => {
                let id = editor::popup_with_options(
                    "API 0.3 demo",
                    PopupOptions {
                        centered: args.iter().any(|arg| arg == "centered"),
                    },
                    |id| {
                        editor::set_status(&format!("closed:{id}"));
                        Ok(())
                    },
                )?;
                editor::set_status(&format!("popup:{id}"));
                if args.iter().any(|arg| arg == "close-now") {
                    editor::close_popup(id);
                }
            }
            "close" => editor::close_popup(number(&args, 0)?),
            "live" => {
                let id = editor::pick_live(
                    Picker::new("API 0.3 queries", vec![Item::text("initial")]),
                    |_, generation, query| {
                        Ok(Some(vec![Item::text(format!("{query}:{generation}"))]))
                    },
                    |choice| {
                        editor::set_status(&format!(
                            "choice:{:?}",
                            choice
                                .items
                                .iter()
                                .map(|item| item.data.as_str())
                                .collect::<Vec<_>>()
                        ));
                        Ok(())
                    },
                )?;
                PICKER.set(Some(id));
                editor::set_status(&format!("picker:{id}"));
            }
            "update" | "push" | "rollback" => {
                let id = PICKER.get().ok_or("no live picker")?;
                let generation = args
                    .first()
                    .ok_or("missing generation")?
                    .parse::<u64>()
                    .map_err(|error| error.to_string())?;
                let items = args.iter().skip(1).map(Item::text).collect();
                if name == "update" {
                    editor::set_items_for_query(id, generation, items)?;
                } else {
                    editor::push_items_for_query(id, generation, items)?;
                }
                if name == "rollback" {
                    return Err("failed after push".into());
                }
            }
            "stream" => {
                let id = editor::spawn_stream(
                    &args,
                    SpawnOptions::default(),
                    |chunk| {
                        editor::set_status(&format!("chunk:{chunk}"));
                        Ok(())
                    },
                    |result| {
                        let output = result?;
                        editor::set_status(&format!(
                            "done:{:?}:{}:{}",
                            output.exit_code, output.stdout, output.stderr
                        ));
                        Ok(())
                    },
                )?;
                editor::set_status(&format!("stream:{id}"));
            }
            "open-at" => {
                let action = match args.get(1).map(String::as_str) {
                    Some("hsplit") => OpenAction::Hsplit,
                    Some("vsplit") => OpenAction::Vsplit,
                    _ => OpenAction::Replace,
                };
                let coordinate = |index| {
                    args.get(index)
                        .map(|_: &String| number(&args, index))
                        .transpose()
                };
                editor::open_file_at(
                    args.first().ok_or("missing path")?,
                    action,
                    coordinate(2)?,
                    coordinate(3)?,
                )?;
            }
            _ => return Err(format!("unknown command {name}")),
        }
        Ok(())
    }

    fn on_post_command(command: &str) {
        editor::log(&format!("event:post:{command}"));
    }
    fn on_focus_gained() {
        editor::log("event:focus:gained");
    }
    fn on_focus_lost() {
        editor::log("event:focus:lost");
    }
    fn on_editor_idle() {
        editor::log("event:idle");
    }
}

fn number(args: &[String], index: usize) -> Result<u32, String> {
    args.get(index)
        .ok_or("missing number")?
        .parse::<u32>()
        .map_err(|error| error.to_string())
}

register_plugin!(Demo);
