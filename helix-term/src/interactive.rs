//! Running interactive terminal programs (e.g. yazi, lazygit) on top of the
//! editor.
//!
//! The program gets the terminal to itself: the application stops reading
//! terminal input and releases raw mode while staying on the alternate screen,
//! so there is no flash of the shell before the program draws. When it exits
//! the editor reclaims the terminal and redraws immediately. Editor state
//! (buffers, jumplist, history, ...) is untouched.

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use helix_view::{editor::Action, Editor};
use tokio::process::Command;

use crate::compositor::Compositor;

pub type OnExit = Box<dyn FnOnce(&mut Editor, &mut Compositor, io::Result<ExitStatus>) + Send>;

pub struct Interactive {
    pub command: Command,
    pub on_exit: OnExit,
}

impl Interactive {
    pub fn new(
        command: Command,
        on_exit: impl FnOnce(&mut Editor, &mut Compositor, io::Result<ExitStatus>) + Send + 'static,
    ) -> Self {
        Self {
            command,
            on_exit: Box::new(on_exit),
        }
    }
}

/// Run `cmd` through the configured shell with the terminal handed over to it.
pub fn shell_command(editor: &Editor, cmd: &str) -> anyhow::Result<Interactive> {
    let shell = &editor.config().shell;
    anyhow::ensure!(!shell.is_empty(), "No shell set");
    let mut command = Command::new(&shell[0]);
    command.args(&shell[1..]).arg(cmd);
    let cmd = cmd.to_owned();
    Ok(Interactive::new(command, move |editor, _, status| {
        report_failure(editor, &cmd, status);
    }))
}

fn report_failure(editor: &mut Editor, program: &str, status: io::Result<ExitStatus>) -> bool {
    match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            match status.code() {
                Some(code) => editor.set_error(format!("'{program}' exited with code {code}")),
                None => editor.set_error(format!("'{program}' was terminated")),
            }
            false
        }
        Err(err) => {
            editor.set_error(format!("Failed to run '{program}': {err}"));
            false
        }
    }
}

/// Pick files with [yazi](https://yazi-rs.github.io), starting at `start`, and
/// open the chosen ones.
pub fn yazi(start: &Path) -> Interactive {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let chooser = std::env::temp_dir().join(format!(
        "helix-yazi-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&chooser);

    let mut command = Command::new("yazi");
    command
        .arg(start)
        .arg(format!("--chooser-file={}", chooser.display()));

    Interactive::new(command, move |editor, _, status| {
        let chosen = std::fs::read_to_string(&chooser).unwrap_or_default();
        let _ = std::fs::remove_file(&chooser);
        // Quitting without choosing anything is not an error.
        if chosen.trim().is_empty() {
            if let Err(err) = &status {
                editor.set_error(format!("Failed to run yazi: {err}"));
            }
            return;
        }
        for path in chosen.lines().filter(|line| !line.is_empty()) {
            let path = PathBuf::from(path);
            if path.is_dir() {
                editor.set_error(format!("'{}' is a directory", path.display()));
                continue;
            }
            if let Err(err) = editor.open(&path, Action::Replace) {
                editor.set_error(format!("Failed to open '{}': {err}", path.display()));
            }
        }
    })
}
