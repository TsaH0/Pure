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
    /// Run on the terminal's normal screen, below the shell's scrollback,
    /// instead of on the editor's alternate screen (like Vim's `:!`).
    pub normal_screen: bool,
    /// Printed before the program runs.
    pub banner: Option<String>,
    /// After the program exits, show its exit status and wait for a key press
    /// before returning to the editor.
    pub wait_for_key: bool,
}

impl Interactive {
    pub fn new(
        command: Command,
        on_exit: impl FnOnce(&mut Editor, &mut Compositor, io::Result<ExitStatus>) + Send + 'static,
    ) -> Self {
        Self {
            command,
            on_exit: Box::new(on_exit),
            normal_screen: false,
            banner: None,
            wait_for_key: false,
        }
    }
}

/// Hand the terminal to `interactive` once the current event is handled.
pub fn request(jobs: &mut crate::job::Jobs, interactive: Interactive) {
    jobs.callback(async move {
        Ok(crate::job::Callback::EditorCompositor(Box::new(
            move |_editor: &mut Editor, compositor: &mut Compositor| {
                compositor.interactive = Some(interactive);
            },
        )))
    });
}

static LAST_COMMAND: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The command last run with [`shell_command_and_wait`].
pub fn last_command() -> Option<String> {
    LAST_COMMAND.lock().unwrap().clone()
}

/// Run `cmd` like Vim's `:!cmd`: the editor steps aside, the command runs on
/// the terminal's normal screen (output scrolls like in a shell and it can
/// read keyboard input), then "Press any key to continue" returns to the
/// editor. An empty `cmd` runs the previous command again.
pub fn shell_command_and_wait(editor: &Editor, cmd: &str) -> anyhow::Result<Interactive> {
    let cmd = if cmd.trim().is_empty() {
        last_command().ok_or_else(|| anyhow::anyhow!("No previous command to run"))?
    } else {
        cmd.to_owned()
    };
    *LAST_COMMAND.lock().unwrap() = Some(cmd.clone());
    let mut interactive = shell_command(editor, &cmd)?;
    // The exit status is shown in the terminal, no need to repeat it.
    interactive.on_exit = Box::new(|_, _, _| {});
    interactive.normal_screen = true;
    interactive.banner = Some(format!("\n:!{cmd}\n"));
    interactive.wait_for_key = true;
    Ok(interactive)
}

/// Print the exit status and wait for a single key press. Called with the
/// terminal in its normal (cooked) state.
pub fn wait_for_key(status: &io::Result<ExitStatus>) {
    use std::io::Write;
    let mut message = String::from("\n");
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => match status.code() {
            Some(code) => message.push_str(&format!("shell returned {code}\n\n")),
            None => message.push_str("shell command was terminated\n\n"),
        },
        Err(err) => message.push_str(&format!("failed to run shell command: {err}\n\n")),
    }
    message.push_str("Press any key to continue");
    let mut stdout = io::stdout();
    let _ = stdout.write_all(message.as_bytes());
    let _ = stdout.flush();
    read_key();
}

#[cfg(unix)]
fn read_key() {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let Ok(mut tty) = std::fs::File::open("/dev/tty") else {
        return;
    };
    let fd = tty.as_raw_fd();
    // SAFETY: plain termios/poll calls on a file descriptor we own.
    unsafe {
        let mut original: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut original) != 0 {
            return;
        }
        let mut raw = original;
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(fd, libc::TCSANOW, &raw);
        let mut buf = [0u8; 64];
        let _ = tty.read(&mut buf);
        // Swallow the rest of a multi-byte key (arrows, ...) so it doesn't
        // reach the editor.
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        while libc::poll(&mut pfd, 1, 20) > 0 {
            if tty.read(&mut buf).unwrap_or(0) == 0 {
                break;
            }
        }
        libc::tcsetattr(fd, libc::TCSANOW, &original);
    }
}

#[cfg(not(unix))]
fn read_key() {
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
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
