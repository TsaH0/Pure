//! Terminals in the editor (`:terminal`): a program on a pseudo-terminal,
//! its screen kept by a terminal emulator (`vt100`) and drawn in place of
//! a document's text. In insert mode keys go to the program (Esc
//! included); `C-\ C-n` goes back to normal mode, as in Neovim.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use helix_event::runtime_local;
use helix_view::graphics::{Color, Modifier, Rect, Style};
use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
use helix_view::{DocumentId, Editor};
use parking_lot::Mutex;
use tui::buffer::Buffer as Surface;

/// Lines kept above the screen.
const SCROLLBACK: usize = 10_000;

pub struct Terminal {
    pub id: u32,
    pub parser: Arc<Mutex<vt100::Parser>>,
    master: File,
    child: Arc<Mutex<Child>>,
    /// The plugin that opened it.
    pub owner: Option<String>,
    /// The program's output ended.
    ended: Arc<AtomicBool>,
    exit: Option<i32>,
    size: (u16, u16),
    /// `C-\` was typed: `C-n` next leaves insert mode.
    escape: bool,
    pub title: String,
}

runtime_local! {
    static TERMINALS: Mutex<BTreeMap<u32, Terminal>> = Mutex::new(BTreeMap::new());
    static NEXT_ID: Mutex<u32> = Mutex::new(1);
}

/// The exit status of `child` once its output `ended`.
fn exit_status(child: &Mutex<Child>, ended: &AtomicBool) -> Option<i32> {
    if !ended.load(Ordering::Relaxed) {
        return None;
    }
    match child.lock().try_wait() {
        Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
        _ => None,
    }
}

/// What the terminals plugins opened show: `(id, owner, read)`, `read`
/// giving the screen's rows and the exit status.
#[allow(clippy::type_complexity)]
pub fn plugin_terminals() -> Vec<(
    u32,
    String,
    Arc<dyn Fn() -> Option<(Vec<String>, Option<i32>)> + Send + Sync>,
)> {
    let terminals = TERMINALS.lock();
    terminals
        .values()
        .filter_map(|terminal| {
            let owner = terminal.owner.clone()?;
            let (parser, child, ended) = (
                terminal.parser.clone(),
                terminal.child.clone(),
                terminal.ended.clone(),
            );
            let read: Arc<dyn Fn() -> Option<(Vec<String>, Option<i32>)> + Send + Sync> =
                Arc::new(move || {
                    let parser = parser.lock();
                    let screen = parser.screen();
                    let rows = screen
                        .rows(0, screen.size().1)
                        .map(|row| row.trim_end().to_string())
                        .collect();
                    Some((rows, exit_status(&child, &ended)))
                });
            Some((terminal.id, owner, read))
        })
        .collect()
}

impl Terminal {
    fn poll_exit(&mut self) {
        if self.exit.is_none() {
            self.exit = exit_status(&self.child, &self.ended);
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.exit.is_none() {
            let _ = self.master.write_all(bytes);
            let _ = self.master.flush();
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        self.parser.lock().screen_mut().set_size(rows, cols);
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCSWINSZ reads a `winsize` from a valid pointer.
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The whole process group: the shell and what it runs.
        let mut child = self.child.lock();
        let pid = child.id() as libc::pid_t;
        // SAFETY: plain signal sends.
        unsafe {
            libc::kill(-pid, libc::SIGHUP);
        }
        let _ = child.kill();
        let _ = child.try_wait();
    }
}

/// Start `argv` (the user's shell when empty) on a pseudo-terminal of
/// `rows` by `cols` in `cwd`, as terminal `id` (a new one when `None`);
/// its id.
pub fn spawn(
    argv: &[String],
    cwd: &Path,
    rows: u16,
    cols: u16,
    id: Option<u32>,
    owner: Option<String>,
) -> anyhow::Result<u32> {
    let (rows, cols) = (rows.max(2), cols.max(2));
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let (program, args) = match argv.split_first() {
        Some((program, args)) => (program.clone(), args.to_vec()),
        None => (shell, Vec::new()),
    };
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty writes two descriptors; the rest may be null.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &size,
        )
    };
    if opened != 0 {
        anyhow::bail!("no pseudo-terminal: {}", std::io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both are open and ours.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let stdio = |fd: &OwnedFd| -> anyhow::Result<Stdio> { Ok(Stdio::from(fd.try_clone()?)) };
    let mut command = Command::new(&program);
    // Programs in it can drive the editor (`pure --remote...`).
    if let Some(socket) = crate::remote::listening() {
        command.env("PURE_LISTEN", socket);
    }
    command
        .args(&args)
        .current_dir(cwd)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(stdio(&slave)?)
        .stdout(stdio(&slave)?)
        .stderr(stdio(&slave)?);
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            // A session of its own, with the pseudo-terminal as its
            // controlling terminal, so job control and ^C work.
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|err| anyhow::anyhow!("can't start {program}: {err}"))?;
    drop(slave);
    let master = File::from(master);
    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
    let ended = Arc::new(AtomicBool::new(false));
    {
        let mut reader = master.try_clone()?;
        let (parser, ended) = (parser.clone(), ended.clone());
        // Redraws go to this editor's runtime.
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("terminal".into())
            .spawn(move || {
                let _runtime = runtime.enter();
                let mut buffer = [0u8; 16 << 10];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            parser.lock().process(&buffer[..n]);
                            helix_event::request_redraw();
                        }
                    }
                }
                ended.store(true, Ordering::Relaxed);
                helix_event::request_redraw();
            })?;
    }
    let id = id.unwrap_or_else(|| {
        let mut next = NEXT_ID.lock();
        *next += 1;
        *next - 1
    });
    let title = Path::new(&program)
        .file_name()
        .map_or(program.clone(), |name| name.to_string_lossy().into_owned());
    TERMINALS.lock().insert(
        id,
        Terminal {
            id,
            parser,
            master,
            child: Arc::new(Mutex::new(child)),
            owner,
            ended,
            exit: None,
            size: (rows, cols),
            escape: false,
            title,
        },
    );
    Ok(id)
}

/// Open a terminal running `argv` (the shell when empty) in a new
/// document, shown as `action` says (a split below, ...); its id.
pub fn open(
    editor: &mut Editor,
    argv: &[String],
    action: helix_view::editor::Action,
) -> anyhow::Result<(u32, DocumentId)> {
    open_as(editor, argv, action, None, None)
}

/// `open`, as terminal `id` of plugin `owner`.
pub fn open_as(
    editor: &mut Editor,
    argv: &[String],
    action: helix_view::editor::Action,
    id: Option<u32>,
    owner: Option<String>,
) -> anyhow::Result<(u32, DocumentId)> {
    let cwd = helix_stdx::env::current_working_dir();
    // The size is put right when it is first drawn.
    let area = editor.tree.get(editor.tree.focus).area;
    let id = spawn(
        argv,
        &cwd,
        area.height.saturating_sub(1),
        area.width,
        id,
        owner,
    )?;
    let title = TERMINALS
        .lock()
        .get(&id)
        .map(|terminal| terminal.title.clone())
        .unwrap_or_default();
    let doc = editor.new_file(action);
    let document = editor.documents.get_mut(&doc).expect("just made");
    document.terminal = Some(id);
    document.name = Some(format!("terminal: {title}").into());
    Ok((id, doc))
}

/// Write `bytes` to terminal `id`.
pub fn write(id: u32, bytes: &[u8]) -> bool {
    match TERMINALS.lock().get_mut(&id) {
        Some(terminal) => {
            terminal.write(bytes);
            true
        }
        None => false,
    }
}

/// End terminal `id` (its program is killed).
pub fn close(id: u32) {
    // Dropped outside the lock: ending the program takes a moment.
    let terminal = TERMINALS.lock().remove(&id);
    drop(terminal);
}

/// Close terminal `id` and its buffer.
pub fn close_with_buffer(editor: &mut Editor, id: u32) {
    let doc = editor
        .documents()
        .find(|doc| doc.terminal == Some(id))
        .map(|doc| doc.id());
    match doc {
        Some(doc) => {
            let _ = editor.close_document(doc, true);
        }
        None => close(id),
    }
}

/// The bytes a terminal program gets for `key`; `None` for keys it
/// can't be sent.
fn encode(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let csi = |code: &str| format!("\x1b[{code}").into_bytes();
    let cursor = |c: char| {
        if application_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let mut bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            let c = c.to_ascii_lowercase();
            match c {
                'a'..='z' => vec![c as u8 - b'a' + 1],
                ' ' | '@' | '2' => vec![0],
                '[' | '3' => vec![0x1b],
                '\\' | '4' => vec![0x1c],
                ']' | '5' => vec![0x1d],
                '^' | '6' => vec![0x1e],
                '_' | '7' | '/' => vec![0x1f],
                _ => return None,
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => csi("Z"),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor('A'),
        KeyCode::Down => cursor('B'),
        KeyCode::Right => cursor('C'),
        KeyCode::Left => cursor('D'),
        KeyCode::Home => cursor('H'),
        KeyCode::End => cursor('F'),
        KeyCode::Insert => csi("2~"),
        KeyCode::Delete => csi("3~"),
        KeyCode::PageUp => csi("5~"),
        KeyCode::PageDown => csi("6~"),
        KeyCode::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            5 => csi("15~"),
            6 => csi("17~"),
            7 => csi("18~"),
            8 => csi("19~"),
            9 => csi("20~"),
            10 => csi("21~"),
            11 => csi("23~"),
            12 => csi("24~"),
            _ => return None,
        },
        _ => return None,
    };
    if alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// The terminal shown by the focused view, if any.
fn focused(editor: &Editor) -> Option<u32> {
    let view = editor.tree.get(editor.tree.focus);
    editor.documents.get(&view.doc)?.terminal
}

/// A key typed in insert mode: sent to the focused terminal. Whether it
/// was one (the editor then does nothing else with it).
pub fn handle_key(editor: &mut Editor, key: KeyEvent) -> bool {
    use helix_view::document::Mode;
    if editor.mode() != Mode::Insert {
        return false;
    }
    let Some(id) = focused(editor) else {
        return false;
    };
    let mut terminals = TERMINALS.lock();
    let Some(terminal) = terminals.get_mut(&id) else {
        return false;
    };
    terminal.poll_exit();
    let ctrl = key.modifiers == KeyModifiers::CONTROL;
    // `C-\ C-n`: back to normal mode.
    if terminal.escape {
        terminal.escape = false;
        if ctrl && key.code == KeyCode::Char('n') {
            drop(terminals);
            editor.mode = Mode::Normal;
            return true;
        }
        terminal.write(&[0x1c]);
    } else if ctrl && key.code == KeyCode::Char('\\') {
        terminal.escape = true;
        return true;
    }
    let application_cursor = {
        let mut parser = terminal.parser.lock();
        // Typing shows the live screen again.
        parser.screen_mut().set_scrollback(0);
        parser.screen().application_cursor()
    };
    if let Some(bytes) = encode(key, application_cursor) {
        terminal.write(&bytes);
    }
    true
}

/// Pasted text, in insert mode: sent to the focused terminal.
pub fn handle_paste(editor: &mut Editor, text: &str) -> bool {
    if editor.mode() != helix_view::document::Mode::Insert {
        return false;
    }
    let Some(id) = focused(editor) else {
        return false;
    };
    let mut terminals = TERMINALS.lock();
    let Some(terminal) = terminals.get_mut(&id) else {
        return false;
    };
    let bracketed = terminal.parser.lock().screen().bracketed_paste();
    let mut bytes = Vec::with_capacity(text.len() + 12);
    if bracketed {
        bytes.extend_from_slice(b"\x1b[200~");
    }
    bytes.extend(text.replace("\r\n", "\r").replace('\n', "\r").into_bytes());
    if bracketed {
        bytes.extend_from_slice(b"\x1b[201~");
    }
    terminal.write(&bytes);
    true
}

/// Scroll terminal `id` back (`lines` > 0) or forward through what has
/// gone off the top.
pub fn scroll(id: u32, lines: i32) {
    if let Some(terminal) = TERMINALS.lock().get(&id) {
        let mut parser = terminal.parser.lock();
        let offset = parser.screen().scrollback() as i64 + lines as i64;
        parser.screen_mut().set_scrollback(offset.max(0) as usize);
    }
}

fn color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

/// Draw terminal `id` in `area` (resizing it to fit); whether there is
/// such a terminal.
pub fn render(
    id: u32,
    area: Rect,
    surface: &mut Surface,
    base: Style,
    cursor: Option<Style>,
) -> bool {
    let mut terminals = TERMINALS.lock();
    let Some(terminal) = terminals.get_mut(&id) else {
        return false;
    };
    terminal.poll_exit();
    // The last row tells that the program ended.
    let rows = if terminal.exit.is_some() {
        area.height.saturating_sub(1)
    } else {
        area.height
    };
    terminal.resize(rows.max(1), area.width.max(1));
    let parser = terminal.parser.lock();
    let screen = parser.screen();
    surface.clear_with(area, base);
    let (screen_rows, screen_cols) = screen.size();
    for row in 0..screen_rows.min(rows) {
        for col in 0..screen_cols.min(area.width) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut style = base;
            if let Some(fg) = color(cell.fgcolor()) {
                style = style.fg(fg);
            }
            if let Some(bg) = color(cell.bgcolor()) {
                style = style.bg(bg);
            }
            let mut modifiers = Modifier::empty();
            if cell.bold() {
                modifiers |= Modifier::BOLD;
            }
            if cell.dim() {
                modifiers |= Modifier::DIM;
            }
            if cell.italic() {
                modifiers |= Modifier::ITALIC;
            }
            if cell.underline() {
                style = style.underline_style(helix_view::graphics::UnderlineStyle::Line);
            }
            if cell.inverse() {
                modifiers |= Modifier::REVERSED;
            }
            style = style.add_modifier(modifiers);
            let contents = cell.contents();
            let symbol = if contents.is_empty() { " " } else { contents };
            let (x, y) = (area.x + col, area.y + row);
            surface[(x, y)].set_symbol(symbol).set_style(style);
        }
    }
    if let Some(cursor) = cursor.filter(|_| !screen.hide_cursor() && screen.scrollback() == 0) {
        let (row, col) = screen.cursor_position();
        if row < rows && col < area.width {
            surface[(area.x + col, area.y + row)].set_style(cursor);
        }
    }
    if let Some(code) = terminal.exit {
        let message = format!("[process exited {code}]");
        surface.set_stringn(
            area.x,
            area.y + rows,
            &message,
            area.width as usize,
            base.add_modifier(Modifier::BOLD),
        );
    }
    true
}

/// End the terminals of closed documents.
pub fn register_hooks() {
    use helix_view::events::DocumentDidClose;
    helix_event::register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if let Some(id) = event.doc.terminal {
            close(id);
        }
        Ok(())
    });
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn keys_become_bytes() {
        let key = |code, modifiers| KeyEvent { code, modifiers };
        let none = KeyModifiers::NONE;
        assert_eq!(encode(key(KeyCode::Char('a'), none), false).unwrap(), b"a");
        assert_eq!(
            encode(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false).unwrap(),
            [3]
        );
        assert_eq!(encode(key(KeyCode::Enter, none), false).unwrap(), b"\r");
        assert_eq!(encode(key(KeyCode::Up, none), false).unwrap(), b"\x1b[A");
        assert_eq!(encode(key(KeyCode::Up, none), true).unwrap(), b"\x1bOA");
        assert_eq!(
            encode(key(KeyCode::Char('x'), KeyModifiers::ALT), false).unwrap(),
            b"\x1bx"
        );
        assert_eq!(encode(key(KeyCode::Esc, none), false).unwrap(), [0x1b]);
    }
}
