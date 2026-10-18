//! An Emacs `M-x compile`-like runner used by `:compile`.
//!
//! The command's output (stdout and stderr) is streamed into a `*compilation*`
//! buffer that is shown in a split below the current view, while focus stays
//! in the editor. The buffer is reused for every run.

use std::borrow::Cow;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use helix_core::{Selection, Transaction};
use helix_view::{editor::Action, DocumentId, Editor, ViewId};
use tokio::process::Command;

use crate::job;

pub const BUFFER_NAME: &str = "*compilation*";

/// Incremented for every run so that output of a superseded run is dropped
/// (and its process killed).
static GENERATION: AtomicU64 = AtomicU64::new(0);
static LAST_COMMAND: Mutex<Option<String>> = Mutex::new(None);

fn find_buffer(editor: &Editor) -> Option<DocumentId> {
    editor
        .documents()
        .find(|doc| doc.name.as_deref() == Some(BUFFER_NAME))
        .map(|doc| doc.id())
}

fn view_showing(editor: &Editor, doc_id: DocumentId) -> Option<ViewId> {
    editor
        .tree
        .views()
        .find(|(view, _)| view.doc == doc_id)
        .map(|(view, _)| view.id)
}

/// Show the compilation buffer in a split below the current view, keeping
/// focus where it is. Returns the buffer, creating it if necessary.
fn show(editor: &mut Editor) -> DocumentId {
    let focus = editor.tree.focus;
    let doc_id = match find_buffer(editor) {
        Some(doc_id) => {
            if view_showing(editor, doc_id).is_none() {
                editor.switch(doc_id, Action::HorizontalSplit);
            }
            doc_id
        }
        None => {
            let doc_id = editor.new_file(Action::HorizontalSplit);
            editor.documents.get_mut(&doc_id).unwrap().name = Some(BUFFER_NAME.into());
            doc_id
        }
    };
    if editor.tree.contains(focus) {
        editor.focus(focus);
    }
    doc_id
}

/// Toggle the window showing the compilation buffer.
pub fn toggle(editor: &mut Editor) {
    if find_buffer(editor).is_none() {
        editor.set_error("No compilation buffer, run a command with :compile first");
        return;
    }
    if !hide(editor) {
        show(editor);
    }
}

/// Close the window showing the compilation buffer (the buffer itself is
/// kept). Returns false if it wasn't shown.
pub fn hide(editor: &mut Editor) -> bool {
    let Some(doc_id) = find_buffer(editor) else {
        return false;
    };
    let Some(view_id) = view_showing(editor, doc_id) else {
        return false;
    };
    if editor.tree.views().count() > 1 {
        editor.close(view_id);
        return true;
    }
    // It is the only window: switch it back to another buffer instead.
    let other = editor
        .tree
        .get(view_id)
        .docs_access_history
        .iter()
        .rev()
        .copied()
        .find(|&id| id != doc_id && editor.documents.contains_key(&id))
        .or_else(|| {
            editor
                .documents()
                .map(|doc| doc.id())
                .find(|&id| id != doc_id)
        });
    match other {
        Some(other) => editor.switch(other, Action::Replace),
        None => {
            editor.new_file(Action::Replace);
        }
    }
    true
}

/// Replace (`replace_all`) or append to the contents of the compilation
/// buffer. A lone `\r` in `text` returns to the start of the last line, so
/// progress output (e.g. `git clone`) overwrites itself like in a terminal.
fn write(editor: &mut Editor, generation: u64, text: &str, replace_all: bool) {
    if GENERATION.load(Ordering::SeqCst) != generation {
        return;
    }
    let Some(doc_id) = find_buffer(editor) else {
        return;
    };
    let doc = editor.documents.get_mut(&doc_id).unwrap();

    // Views whose cursor is on the last line follow the output.
    let following: Vec<ViewId> = doc
        .selections()
        .iter()
        .filter(|(_, selection)| {
            let text = doc.text().slice(..);
            replace_all
                || text.char_to_line(selection.primary().cursor(text))
                    >= text.len_lines().saturating_sub(2)
        })
        .map(|(&view_id, _)| view_id)
        .collect();
    let Some(&any_view) = doc.selections().keys().next() else {
        return;
    };

    let mut pieces = text.split('\r');
    let mut edits = Vec::new();
    if let Some(first) = pieces.next() {
        let from = if replace_all {
            0
        } else {
            doc.text().len_chars()
        };
        edits.push((from, first));
    }
    for piece in pieces {
        // Placeholder: the start of the last line is only known once the
        // previous edit has been applied.
        edits.push((usize::MAX, piece));
    }

    for (from, piece) in edits {
        let text = doc.text();
        let to = text.len_chars();
        let from = if from == usize::MAX {
            text.line_to_char(text.char_to_line(to))
        } else {
            from
        };
        // An empty piece after `\r` means nothing was written over the
        // line yet, so keep it (like a terminal does).
        if piece.is_empty() && !replace_all {
            continue;
        }
        let transaction =
            Transaction::change(text, std::iter::once((from, to, Some(piece.into()))));
        doc.apply_output(&transaction, any_view);
        for (view, _) in editor.tree.views_mut() {
            view.apply(&transaction, doc);
        }
    }

    let end = doc.text().len_chars();
    for view_id in following {
        doc.set_selection(view_id, Selection::point(end));
        if replace_all {
            // A new run starts at the top. The view may not have been laid
            // out yet, so don't try to scroll it.
            doc.set_view_offset(view_id, Default::default());
        } else if editor.tree.contains(view_id) {
            // Follow the output, but only scroll once it no longer fits.
            editor.tree.get(view_id).ensure_cursor_in_view(doc, 0);
        }
    }
}

/// Remove ANSI escape sequences (colors, cursor movement, ...).
fn strip_ansi(text: &str) -> Cow<'_, str> {
    if !text.contains('\x1b') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ params final-byte
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... (BEL | ESC \)
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    Cow::Owned(out)
}

/// Turns raw process output into text for the buffer. Handles UTF-8 sequences
/// and `\r\n` split across reads.
#[derive(Default)]
struct Decoder {
    bytes: Vec<u8>,
}

impl Decoder {
    fn push(&mut self, chunk: &[u8], eof: bool) -> String {
        self.bytes.extend_from_slice(chunk);
        let mut valid = match std::str::from_utf8(&self.bytes) {
            Ok(_) => self.bytes.len(),
            // An incomplete sequence at the end: wait for the rest of it.
            Err(err) if err.error_len().is_none() && !eof => err.valid_up_to(),
            Err(_) => self.bytes.len(),
        };
        // Keep a trailing `\r` in case the next read starts with `\n`.
        if !eof && valid > 0 && self.bytes[valid - 1] == b'\r' {
            valid -= 1;
        }
        let rest = self.bytes.split_off(valid);
        let bytes = std::mem::replace(&mut self.bytes, rest);
        let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        strip_ansi(&text).into_owned()
    }
}

/// Read the combined stdout/stderr of the process and stream it into the
/// buffer. Runs on a blocking thread.
fn pump(mut reader: impl std::io::Read, generation: u64) {
    let mut buf = vec![0u8; 16 * 1024];
    let mut decoder = Decoder::default();
    loop {
        let (n, eof) = match reader.read(&mut buf) {
            Ok(0) => (0, true),
            Ok(n) => (n, false),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => (0, true),
        };
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        let text = decoder.push(&buf[..n], eof);
        if !text.is_empty() {
            job::dispatch_blocking(move |editor, _| write(editor, generation, &text, false));
        }
        if eof {
            return;
        }
    }
}

/// Kill the process and everything it started (it runs in its own process
/// group), like Emacs' `kill-compilation`.
fn kill(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: sending a signal has no memory safety implications.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        }
        return;
    }
    let _ = child.start_kill();
}

#[cfg(unix)]
fn now() -> String {
    // SAFETY: `localtime_r` and `strftime` only write into the buffers we pass.
    unsafe {
        let time = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&time, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 64];
        let len = libc::strftime(
            buf.as_mut_ptr().cast(),
            buf.len(),
            c"%a %b %e %H:%M:%S".as_ptr(),
            &tm,
        );
        String::from_utf8_lossy(&buf[..len]).into_owned()
    }
}

#[cfg(not(unix))]
fn now() -> String {
    String::new()
}

fn at_now() -> String {
    let now = now();
    if now.is_empty() {
        now
    } else {
        format!(" at {now}")
    }
}

/// The command run last, offered as the default by the `compile` prompt.
pub fn last_command() -> Option<String> {
    LAST_COMMAND.lock().unwrap().clone()
}

/// Run `cmd` (or the previous command if `cmd` is empty) and show its output
/// in the compilation buffer.
pub fn compile(editor: &mut Editor, cmd: &str) -> anyhow::Result<()> {
    let cmd = if cmd.trim().is_empty() {
        LAST_COMMAND
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No previous command to run"))?
    } else {
        cmd.to_owned()
    };
    *LAST_COMMAND.lock().unwrap() = Some(cmd.clone());

    let shell = editor.config().shell.clone();
    anyhow::ensure!(!shell.is_empty(), "No shell set");
    let cwd: PathBuf = helix_stdx::env::current_working_dir();
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    show(editor);
    let header = format!(
        "-*- mode: compilation; default-directory: \"{}/\" -*-\nCompilation started{}\n\n{}\n",
        cwd.display(),
        at_now(),
        cmd
    );
    write(editor, generation, &header, true);
    editor.set_status(format!("Running: {cmd}"));

    // stdout and stderr share one pipe so their output stays in order, as in
    // a terminal.
    let (reader, writer) = std::io::pipe()?;
    let mut process = Command::new(&shell[0]);
    process
        .args(&shell[1..])
        .arg(&cmd)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer.try_clone()?))
        .stderr(Stdio::from(writer))
        .kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);

    tokio::spawn(async move {
        let started = Instant::now();
        let spawned = process.spawn();
        // Close our copies of the pipe's write end so the reader sees EOF
        // once the process exits.
        drop(process);
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) => {
                let message = format!("Failed to start shell: {err}\n");
                job::dispatch(move |editor, _| {
                    write(editor, generation, &message, false);
                    editor.set_error(message.trim_end().to_owned());
                })
                .await;
                return;
            }
        };
        let output = tokio::task::spawn_blocking(move || pump(reader, generation));

        let superseded = async {
            while GENERATION.load(Ordering::SeqCst) == generation {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        let status = tokio::select! {
            status = child.wait() => status,
            _ = superseded => {
                kill(&mut child);
                return;
            }
        };
        // Let the reader drain what is left. Don't wait forever: a background
        // process started by the command may keep the pipe open.
        let _ = tokio::time::timeout(Duration::from_secs(1), output).await;

        let duration = format!("duration {:.2} s", started.elapsed().as_secs_f64());
        let (footer, ok) = match status {
            Ok(status) if status.success() => (
                format!("Compilation finished{}, {duration}", at_now()),
                true,
            ),
            Ok(status) => match status.code() {
                Some(code) => (
                    format!(
                        "Compilation exited abnormally with code {code}{}, {duration}",
                        at_now()
                    ),
                    false,
                ),
                None => (format!("Compilation killed{}, {duration}", at_now()), false),
            },
            Err(err) => (format!("Compilation failed: {err}"), false),
        };
        job::dispatch(move |editor, _| {
            let separator = editor
                .documents
                .values()
                .find(|doc| doc.name.as_deref() == Some(BUFFER_NAME))
                .map_or("\n", |doc| {
                    if doc.text().len_chars() == 0
                        || doc.text().slice(..).chars().last() == Some('\n')
                    {
                        "\n"
                    } else {
                        "\n\n"
                    }
                });
            write(editor, generation, &format!("{separator}{footer}\n"), false);
            if GENERATION.load(Ordering::SeqCst) == generation {
                if ok {
                    editor.set_status(footer);
                } else {
                    editor.set_error(footer);
                }
            }
        })
        .await;
    });

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi("\x1b[1;31merror\x1b[0m: x"), "error: x");
        assert_eq!(strip_ansi("\x1b]8;;http://a\x1b\\link\x1b]8;;\x07"), "link");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn decoder_handles_split_input() {
        let mut decoder = Decoder::default();
        // "é" split across two reads, and "\r\n" split across two reads.
        assert_eq!(decoder.push(&[b'a', 0xc3], false), "a");
        assert_eq!(decoder.push(&[0xa9, b'\r'], false), "é");
        assert_eq!(decoder.push(b"\nb", false), "\nb");
        assert_eq!(decoder.push(b"\r", false), "");
        assert_eq!(decoder.push(b"", true), "\r");
    }
}
