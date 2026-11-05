//! The debugger for plugins (API 0.5 `debug`): its state in snapshots,
//! raw requests, breakpoints and events.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use helix_event::runtime_local;
use helix_plugin::v05::{BreakpointInfo, Callback2, DebugSession, DebugState, StackFrame};
use helix_view::editor::Breakpoint;
use helix_view::Editor;
use parking_lot::Mutex;

runtime_local! {
    /// Whether a loaded plugin has the `debug` permission.
    pub static WANTED: AtomicBool = AtomicBool::new(false);
    /// The debugger events each plugin hears.
    static LISTENERS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());
}

fn thread_number(thread: helix_dap::ThreadId) -> u64 {
    thread.to_string().parse::<i64>().unwrap_or(0) as u64
}

/// What plugins see of the debugger; `None` while no plugin may look.
pub fn state(editor: &Editor) -> Option<DebugState> {
    if !WANTED.load(Ordering::Relaxed) {
        return None;
    }
    let session = editor.debug_adapters.get_active_client().map(|client| {
        let frames = client
            .thread_id
            .and_then(|thread| client.stack_frames.get(&thread))
            .map(|frames| {
                frames
                    .iter()
                    .map(|frame| StackFrame {
                        id: frame.id as u64,
                        name: frame.name.clone(),
                        path: frame
                            .source
                            .as_ref()
                            .and_then(|source| source.path.as_ref())
                            .map(|path| path.display().to_string()),
                        line: frame.line.saturating_sub(1) as u32,
                        column: frame.column.saturating_sub(1) as u32,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut threads: Vec<_> = client
            .thread_states
            .iter()
            .map(|(thread, state)| (thread_number(*thread), state.clone()))
            .collect();
        threads.sort();
        DebugSession {
            adapter: client
                .config
                .as_ref()
                .map(|config| config.name.clone())
                .unwrap_or_default(),
            thread: client.thread_id.map(thread_number),
            threads,
            frames,
            active_frame: client.active_frame.map(|frame| frame as u32),
        }
    });
    let mut breakpoints: Vec<_> = editor
        .breakpoints
        .iter()
        .flat_map(|(path, breakpoints)| {
            breakpoints.iter().map(move |breakpoint| BreakpointInfo {
                path: path.display().to_string(),
                line: breakpoint.line as u32,
                condition: breakpoint.condition.clone(),
                log_message: breakpoint.log_message.clone(),
                verified: breakpoint.verified,
            })
        })
        .collect();
    breakpoints.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    Some(DebugState {
        session,
        breakpoints,
    })
}

/// Send `command` to the running debugger; the reply goes to `plugin`.
pub fn request(editor: &Editor, plugin: String, id: u32, command: String, arguments: String) {
    let arguments = serde_json::from_str(&arguments).ok();
    let Some(call) = editor
        .debug_adapters
        .get_active_client()
        .map(|client| client.call_raw(command, arguments))
    else {
        let result = Err("no debug session is running".to_string());
        return super::deliver2_later(plugin, Callback2::LspReply { id, result });
    };
    tokio::spawn(async move {
        let result = match call.await {
            Ok(body) => {
                let json = body.to_string();
                if json.len() > 16 << 20 {
                    Err("the reply is larger than 16 MiB".to_string())
                } else {
                    Ok(json)
                }
            }
            Err(err) => Err(err.to_string()),
        };
        super::deliver2_later(plugin, Callback2::LspReply { id, result });
    });
}

/// Make `breakpoints` those of `path`, and tell a running debugger.
pub fn set_breakpoints(editor: &mut Editor, path: PathBuf, breakpoints: Vec<BreakpointInfo>) {
    let mut breakpoints: Vec<Breakpoint> = breakpoints
        .into_iter()
        .map(|breakpoint| Breakpoint {
            line: breakpoint.line as usize,
            condition: breakpoint.condition,
            log_message: breakpoint.log_message,
            ..Default::default()
        })
        .collect();
    breakpoints.sort_by_key(|breakpoint| breakpoint.line);
    breakpoints.dedup_by_key(|breakpoint| breakpoint.line);
    if let Some(debugger) = editor.debug_adapters.get_active_client_mut() {
        if let Err(err) =
            helix_view::handlers::dap::breakpoints_changed(debugger, path.clone(), &mut breakpoints)
        {
            editor.set_error(format!("Failed to set breakpoints: {err}"));
        }
    }
    if breakpoints.is_empty() {
        editor.breakpoints.remove(&path);
    } else {
        editor.breakpoints.insert(path, breakpoints);
    }
    helix_event::request_redraw();
}

pub fn listen(plugin: String, events: Vec<String>) {
    let mut listeners = LISTENERS.lock();
    listeners.retain(|(owner, _)| *owner != plugin);
    if !events.is_empty() {
        listeners.push((plugin, events));
    }
}

/// Forget `plugin`'s listening (`None`: everyone's).
pub fn stop_plugin(plugin: Option<&str>) {
    LISTENERS
        .lock()
        .retain(|(owner, _)| plugin.is_some_and(|plugin| plugin != owner));
}

/// The debugger sent event `name`: tell the plugins listening, after it
/// is handled.
pub fn event(name: &str, body: Option<&serde_json::Value>) {
    let plugins: Vec<String> = LISTENERS
        .lock()
        .iter()
        .filter(|(_, events)| events.iter().any(|e| e == name || e == "*"))
        .map(|(plugin, _)| plugin.clone())
        .collect();
    if plugins.is_empty() {
        return;
    }
    let payload = body.map_or_else(|| "null".to_string(), |body| body.to_string());
    for plugin in plugins {
        let call = super::editor_call(&plugin, "debug-event", name, payload.clone());
        tokio::spawn(crate::job::dispatch_callback(
            crate::job::Callback::EditorCompositorJobs(Box::new(move |editor, _, jobs| {
                if let Err(err) = super::serve_now(editor, jobs, &call) {
                    log::warn!("{plugin}: debug event: {err}");
                }
            })),
        ));
    }
}
