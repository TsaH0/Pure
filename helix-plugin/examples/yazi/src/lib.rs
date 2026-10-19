//! Example plugin: pick files with [yazi](https://yazi-rs.github.io).
//!
//! `:plugin-run yazi/open` hands the terminal to yazi, starting at the current
//! file (or the working directory), and opens the files you choose. Bind it:
//!
//! ```toml
//! # config.toml
//! [keys.normal.space]
//! e = ":plugin-run yazi/open"
//! ```
//!
//! It needs the user's grant in `plugins.toml`:
//!
//! ```toml
//! [yazi]
//! allow-process = ["yazi"]
//! ```

use helix_plugin_api::{
    editor, register_plugin, CommandInfo, InteractiveOptions, Plugin, Registration,
};

struct Yazi;

impl Plugin for Yazi {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "open".into(),
                doc: "Pick files with yazi and open them".into(),
            }],
            events: vec![],
        })
    }

    fn run_command(name: &str, _args: Vec<String>) -> Result<(), String> {
        if name != "open" {
            return Err(format!("unknown command {name}"));
        }
        let start = editor::current_document()
            .and_then(|doc| editor::document_path(doc).ok().flatten())
            .unwrap_or_else(editor::cwd);
        editor::run_interactive(
            &["yazi", &start, "--chooser-file={chooser}"],
            InteractiveOptions {
                chooser_file: true,
                ..Default::default()
            },
            |output| {
                let output = output?;
                // Quitting without choosing anything is fine.
                for path in output.stdout.lines().filter(|line| !line.is_empty()) {
                    editor::open_file(path)?;
                }
                Ok(())
            },
        )?;
        Ok(())
    }
}

register_plugin!(Yazi);
