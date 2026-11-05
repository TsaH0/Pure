//! Driving a running editor from other programs (`pure --listen`), like
//! Neovim's `--listen` and `$NVIM`. A Unix socket only its user can use
//! takes JSON requests, one per line, and answers each with one line:
//!
//! - `{"id": 1, "method": "command", "params": {"line": "open src/main.rs"}}`
//!   runs a command line, as typed after `:`;
//! - `{"method": "keys", "params": {"keys": "ggdd"}}` plays keys as if typed;
//! - `{"method": "open", "params": {"paths": ["a.rs", "b.rs"]}}` opens files;
//! - `{"method": "state"}` tells the mode, the current file, the cursor
//!   and the open documents.
//!
//! The answer is `{"id": 1, "result": ...}` or `{"id": 1, "error": "..."}`.
//! Programs in the editor's terminals find the socket in `$PURE_LISTEN`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{bail, Context};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use crate::compositor::Compositor;
use crate::job::{self, Jobs};
use helix_view::Editor;

/// The socket this editor listens on.
static LISTENING: OnceLock<PathBuf> = OnceLock::new();

/// Longest request line accepted.
const MAX_REQUEST: usize = 1 << 20;

pub fn listening() -> Option<&'static Path> {
    LISTENING.get().map(PathBuf::as_path)
}

/// Where `--listen` without a path puts the socket: a directory of the
/// user's own, the socket named after the process.
pub fn default_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("pure"))
        // SAFETY: getuid can't fail.
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("pure-{}", unsafe { libc::getuid() }))
        });
    dir.join(format!("pure-{}.sock", std::process::id()))
}

/// Listen on `path`; requests are served by the editor's event loop.
pub fn listen(path: PathBuf) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    // A socket left by an editor that is gone (nothing answers on it).
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            bail!("another editor listens on {}", path.display());
        }
        std::fs::remove_file(&path)?;
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("can't listen on {}", path.display()))?;
    // Only this user may drive the editor.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let _ = LISTENING.set(path);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve(stream));
        }
    });
    Ok(())
}

/// Remove the socket (when the editor quits).
pub fn stop() {
    if let Some(path) = listening() {
        let _ = std::fs::remove_file(path);
    }
}

async fn serve(stream: UnixStream) {
    let (read, mut write) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let reply = if line.len() > MAX_REQUEST {
            json!({ "id": null, "error": "the request is too long" })
        } else {
            match serde_json::from_str::<Value>(&line) {
                Ok(request) => {
                    let id = request.get("id").cloned().unwrap_or(Value::Null);
                    match run(request).await {
                        Ok(result) => json!({ "id": id, "result": result }),
                        Err(err) => json!({ "id": id, "error": err }),
                    }
                }
                Err(err) => json!({ "id": null, "error": format!("not JSON: {err}") }),
            }
        };
        let mut reply = reply.to_string();
        reply.push('\n');
        if write.write_all(reply.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// Do `request` on the editor's thread; its result.
async fn run(request: Value) -> Result<Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    job::dispatch_callback(job::Callback::EditorCompositorJobs(Box::new(
        move |editor, compositor, jobs| {
            let _ = tx.send(handle(editor, compositor, jobs, &request));
        },
    )))
    .await;
    rx.await
        .unwrap_or_else(|_| Err("the editor is gone".into()))
}

fn handle(
    editor: &mut Editor,
    compositor: &mut Compositor,
    jobs: &mut Jobs,
    request: &Value,
) -> Result<Value, String> {
    let params = &request["params"];
    let string = |name: &str| {
        params[name]
            .as_str()
            .ok_or_else(|| format!("params.{name} must be a string"))
    };
    let method = request["method"].as_str().ok_or("no method")?;
    let mut cx = crate::compositor::Context {
        editor,
        jobs,
        scroll: None,
    };
    match method {
        "command" => {
            let line = string("line")?;
            let line = line.strip_prefix(':').unwrap_or(line);
            crate::commands::typed::run_command_line(&mut cx, line)
                .map_err(|err| err.to_string())?;
            Ok(Value::Null)
        }
        "keys" => {
            let keys =
                helix_view::input::parse_macro(string("keys")?).map_err(|err| err.to_string())?;
            crate::commands::feed_keys(compositor, &mut cx, &keys, false)
                .map_err(|err| err.to_string())?;
            Ok(Value::Null)
        }
        "open" => {
            let paths = params["paths"]
                .as_array()
                .ok_or("params.paths must be a list")?;
            let cwd = helix_stdx::env::current_working_dir();
            for path in paths {
                let path = path.as_str().ok_or("paths are strings")?;
                let path = helix_stdx::path::canonicalize(cwd.join(path));
                cx.editor
                    .open(&path, helix_view::editor::Action::Replace)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
            }
            Ok(Value::Null)
        }
        "state" => Ok(state(cx.editor)),
        method => Err(format!(
            "unknown method '{method}': command, keys, open or state"
        )),
    }
}

fn state(editor: &Editor) -> Value {
    let (view, doc) = helix_view::current_ref!(editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    let position = helix_core::coords_at_pos(text, cursor);
    let mode = match editor.mode() {
        helix_view::document::Mode::Normal => "normal",
        helix_view::document::Mode::Insert => "insert",
        helix_view::document::Mode::Select => "select",
    };
    json!({
        "mode": mode,
        "file": doc.path(),
        "modified": doc.is_modified(),
        "line": position.row + 1,
        "column": position.col + 1,
        "selection": doc.selection(view.id).primary().fragment(text),
        "documents": editor
            .documents()
            .map(|doc| json!({ "path": doc.path(), "modified": doc.is_modified() }))
            .collect::<Vec<_>>(),
        "cwd": helix_stdx::env::current_working_dir(),
    })
}

/// Send `request` to the editor listening on `path` and wait for its
/// answer (`pure --remote...`).
pub fn request(path: &Path, request: Value) -> anyhow::Result<Value> {
    let mut stream = std::os::unix::net::UnixStream::connect(path)
        .with_context(|| format!("no editor listens on {}", path.display()))?;
    let mut line = request.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let reply: Value = serde_json::from_str(&reply).context("the editor's answer isn't JSON")?;
    match reply.get("error") {
        Some(error) => bail!("{}", error.as_str().unwrap_or("failed")),
        None => Ok(reply["result"].clone()),
    }
}
