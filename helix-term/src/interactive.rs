//! Running interactive terminal programs (e.g. lazygit, a file manager) on top of the
//! editor.
//!
//! The program gets the terminal to itself: the application stops reading
//! terminal input and releases raw mode while staying on the alternate screen,
//! so there is no flash of the shell before the program draws. When it exits
//! the editor reclaims the terminal and redraws immediately. Editor state
//! (buffers, jumplist, history, ...) is untouched.

use std::io;
use std::process::ExitStatus;

use helix_view::Editor;
use tokio::process::Command;

use crate::compositor::Compositor;

pub type OnExit = Box<dyn FnOnce(&mut Editor, &mut Compositor, io::Result<ExitStatus>) + Send>;

pub struct Interactive {
    pub command: Command,
    pub on_exit: OnExit,
    /// Optional validity check for a deferred request, before terminal handover.
    pub before_run: Option<Box<dyn Fn() -> bool + Send>>,
    /// Run on the terminal's normal screen, below the shell's scrollback,
    /// instead of on the editor's alternate screen (like Vim's `:!`).
    pub normal_screen: bool,
    /// Clear the screen before the program runs.
    pub clear_screen: bool,
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
            before_run: None,
            normal_screen: false,
            clear_screen: false,
            banner: None,
            wait_for_key: false,
        }
    }

    pub fn can_run(&self) -> bool {
        self.before_run.as_ref().is_none_or(|check| check())
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

/// Run `cmd` like Vim's `:!cmd`: the editor steps aside, the screen is cleared
/// and the command runs there (output scrolls like in a shell and it can read
/// keyboard input), then "Press any key to continue" returns to the editor.
/// An empty `cmd` runs the previous command again.
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
    // Only the program's output is shown on the cleared screen.
    interactive.clear_screen = true;
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
