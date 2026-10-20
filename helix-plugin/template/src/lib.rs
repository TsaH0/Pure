//! {{description}}

use helix_plugin_api::{
    editor, register_plugin, CommandInfo, DocumentId, Edit, EventKind, Plugin, Registration,
};

struct {{crate_name | pascal_case}};

impl Plugin for {{crate_name | pascal_case}} {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "hello".into(),
                doc: "Insert a greeting at the cursor".into(),
            }],
            events: vec![EventKind::DocumentOpen],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        match name {
            "hello" => {
                let doc = editor::current_document().ok_or("no document")?;
                let cursor = editor::selections(doc)?[0].head;
                editor::apply_edits(
                    doc,
                    &[Edit {
                        from: cursor,
                        to: cursor,
                        text: "Hello from {{project-name}}!".into(),
                    }],
                )
            }
            _ => Err(format!("unknown command {name}")),
        }
    }

    fn on_document_open(doc: DocumentId) {
        if let Ok(Some(path)) = editor::document_path(doc) {
            editor::log(&format!("opened {path}"));
        }
    }
}

register_plugin!({{crate_name | pascal_case}});
