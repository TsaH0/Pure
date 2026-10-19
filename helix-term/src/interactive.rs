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
    /// Clear the screen before the program runs.
    pub clear_screen: bool,
    /// Printed before the program runs.
    pub banner: Option<String>,
    /// After the program exits, show its exit status and wait for a key press
    /// before returning to the editor.
    pub wait_for_key: bool,
    /// Revalidate deferred plugin requests immediately before terminal release.
    pub before_start: Option<Box<dyn FnOnce() -> bool + Send>>,
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
            clear_screen: false,
            banner: None,
            wait_for_key: false,
            before_start: None,
        }
    }
}

/// Hand the terminal to `interactive` once the current event is handled.
pub fn request(jobs: &mut crate::job::Jobs, interactive: Interactive) {
    request_if(jobs, interactive, |_, _| true);
}

pub(crate) fn request_if(
    jobs: &mut crate::job::Jobs,
    interactive: Interactive,
    ready: impl FnOnce(&Editor, &Compositor) -> bool + Send + 'static,
) {
    jobs.callback(async move {
        Ok(crate::job::Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                if ready(editor, compositor) {
                    compositor.interactive = Some(interactive);
                }
            },
        )))
    });
}

/// Build a direct program invocation with a securely allocated chooser file.
/// Substitute `{chooser}` in arguments; the caller keeps the returned path
/// alive until exit, and dropping it also cleans up requests never launched.
pub fn with_chooser(
    argv: &[String],
    cwd: Option<&Path>,
) -> io::Result<(Interactive, tempfile::TempPath)> {
    let program = argv
        .first()
        .filter(|program| !program.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty interactive argv"))?;
    let directory = std::path::absolute(std::env::temp_dir())?;
    let chooser = tempfile::Builder::new()
        .prefix("pure-chooser-")
        .tempfile_in(directory)?
        .into_temp_path();
    let path = chooser
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "chooser path is not UTF-8"))?;
    let mut command = Command::new(program);
    command.args(argv[1..].iter().map(|arg| arg.replace("{chooser}", path)));
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    Ok((Interactive::new(command, |_, _, _| {}), chooser))
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

/// Pick files with [yazi](https://yazi-rs.github.io), starting at `start`, and
/// open the chosen ones.
pub fn yazi(start: &Path) -> io::Result<Interactive> {
    let argv = vec!["yazi".into(), "--chooser-file={chooser}".into()];
    let (mut interactive, chooser) = with_chooser(&argv, None)?;
    // Keep the original OsStr path, including non-UTF-8 file names.
    interactive.command.arg(start);
    interactive.on_exit = Box::new(move |editor, _, status| {
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
    });
    Ok(interactive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooser_arguments_are_substituted_and_owned_path_cleans_up() {
        let cwd = tempfile::tempdir().unwrap();
        let argv = vec![
            "program".into(),
            "{chooser}".into(),
            "--chooser-file={chooser}".into(),
            "literal argument".into(),
        ];
        let (interactive, chooser) = with_chooser(&argv, Some(cwd.path())).unwrap();
        let path = chooser.to_path_buf();
        let args: Vec<_> = interactive
            .command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                path.to_str().unwrap().to_owned(),
                format!("--chooser-file={}", path.display()),
                "literal argument".into()
            ]
        );
        assert_eq!(interactive.command.as_std().get_program(), "program");
        assert_eq!(
            interactive.command.as_std().get_current_dir(),
            Some(cwd.path())
        );
        assert!(path.is_file());
        drop(interactive);
        drop(chooser);
        assert!(!path.exists());
        assert!(with_chooser(&[], None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn yazi_keeps_non_utf8_start_paths() {
        use std::os::unix::ffi::OsStringExt;
        let start = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/pure-\xff".to_vec()));
        let interactive = yazi(&start).unwrap();
        assert_eq!(
            interactive.command.as_std().get_args().last(),
            Some(start.as_os_str())
        );
        assert!(!interactive.normal_screen);
        assert!(!interactive.wait_for_key);
    }
}
