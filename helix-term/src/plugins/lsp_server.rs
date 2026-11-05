//! Plugins as and around language servers (API 0.5 `lsp.serve`, `listen`,
//! `filter`, `start`).
//!
//! A plugin's own server runs in-process: `helix_lsp` talks LSP over an
//! in-memory stream to [`run`], which answers `initialize` with the
//! plugin's capabilities and passes every other request to the plugin's
//! "lsp" service on the main thread.

use std::collections::HashMap;

use helix_event::runtime_local;
use helix_lsp::{AddedServer, IN_PROCESS};
use helix_plugin::v05::{LspAction, ServiceCall};
use helix_view::Editor;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::job;

runtime_local! {
    /// Plugins serving as language servers: server name -> (plugin, capabilities).
    static SERVERS: Mutex<std::collections::BTreeMap<String, (String, Value)>> = Mutex::new(std::collections::BTreeMap::new());
    /// Notification methods each plugin hears.
    static LISTENERS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());
    /// Request methods whose results each plugin changes.
    static FILTERS: Mutex<Vec<(String, Vec<String>)>> = Mutex::new(Vec::new());
}

/// Hook the in-process servers and reply filters into `helix_lsp` (once).
pub fn install() {
    helix_lsp::set_in_process_server(start_server);
    helix_lsp::set_reply_filter(helix_lsp::ReplyFilter {
        wants: |_server, method| {
            FILTERS
                .lock()
                .iter()
                .any(|(_, methods)| methods.iter().any(|m| m == method))
        },
        filter: filter_reply,
    });
}

/// Do what a plugin asked.
pub fn apply(editor: &mut Editor, action: LspAction) {
    match action {
        LspAction::Serve {
            plugin,
            languages,
            capabilities,
        } => {
            let capabilities = serde_json::from_str(&capabilities).unwrap_or(Value::Null);
            SERVERS
                .lock()
                .insert(plugin.clone(), (plugin.clone(), capabilities));
            add(
                editor,
                AddedServer {
                    name: plugin.clone(),
                    languages,
                    config: server_config(format!("{IN_PROCESS}{plugin}"), Vec::new(), None),
                },
            );
        }
        LspAction::Listen { plugin, methods } => {
            let mut listeners = LISTENERS.lock();
            listeners.retain(|(owner, _)| *owner != plugin);
            listeners.push((plugin, methods));
        }
        LspAction::Filter { plugin, methods } => {
            let mut filters = FILTERS.lock();
            filters.retain(|(owner, _)| *owner != plugin);
            filters.push((plugin, methods));
        }
        LspAction::Start {
            plugin: _,
            name,
            command,
            args,
            languages,
            config,
        } => {
            let config = config.and_then(|config| serde_json::from_str(&config).ok());
            add(
                editor,
                AddedServer {
                    name,
                    languages,
                    config: server_config(command, args, config),
                },
            );
        }
    }
}

fn server_config(
    command: String,
    args: Vec<String>,
    config: Option<Value>,
) -> helix_core::syntax::config::LanguageServerConfiguration {
    helix_core::syntax::config::LanguageServerConfiguration {
        command,
        args,
        environment: HashMap::new(),
        config,
        timeout: 20,
        required_root_patterns: None,
    }
}

/// Add `server` and attach it to the open documents in its languages.
fn add(editor: &mut Editor, server: AddedServer) {
    let languages = server.languages.clone();
    editor.language_servers.add_server(server);
    let docs: Vec<_> = editor
        .documents()
        .filter(|doc| {
            doc.language_config()
                .is_some_and(|config| languages.contains(&config.language_id))
        })
        .map(|doc| doc.id())
        .collect();
    for doc in docs {
        editor.refresh_language_servers(doc);
    }
}

/// Forget `plugin`'s servers, listeners and filters (`None`: everyone's).
pub fn stop_plugin(editor: &mut Editor, plugin: Option<&str>) {
    let mine = |owner: &str| plugin.is_none_or(|plugin| plugin == owner);
    let names: Vec<String> = SERVERS
        .lock()
        .iter()
        .filter(|(_, (owner, _))| mine(owner))
        .map(|(name, _)| name.clone())
        .collect();
    for name in names {
        SERVERS.lock().remove(&name);
        editor.language_servers.remove_server(&name);
    }
    LISTENERS.lock().retain(|(owner, _)| !mine(owner));
    FILTERS.lock().retain(|(owner, _)| !mine(owner));
}

/// A notification from server `server`: pass it to the plugins listening.
pub fn notification(server: &str, method: &str, params: &Value) {
    let listeners: Vec<String> = LISTENERS
        .lock()
        .iter()
        .filter(|(_, methods)| methods.iter().any(|m| m == method || m == "*"))
        .map(|(plugin, _)| plugin.clone())
        .collect();
    if listeners.is_empty() {
        return;
    }
    let payload = json!({ "server": server, "params": params }).to_string();
    for plugin in listeners {
        let call = super::editor_call(&plugin, "lsp-notification", method, payload.clone());
        tokio::spawn(async move {
            let _ = serve(call).await;
        });
    }
}

/// Run `call` on the main thread; its reply.
async fn serve(call: ServiceCall) -> Result<String, String> {
    let (tx, rx) = oneshot::channel();
    job::dispatch_callback(job::Callback::EditorCompositorJobs(Box::new(
        move |editor, _, jobs| {
            let snapshot = super::snapshot(editor);
            let result = super::with_host(|host| host.serve(&call, &snapshot));
            let reply = match result {
                Some((reply, actions)) => {
                    super::apply(editor, Some(jobs), actions);
                    reply
                }
                None => Err("plugins aren't loaded".into()),
            };
            let _ = tx.send(reply);
        },
    )))
    .await;
    rx.await
        .unwrap_or_else(|_| Err("the editor is gone".into()))
}

/// Pass the result of a `method` request to `server` through the plugins
/// filtering it, then on to `reply`.
fn filter_reply(
    server: String,
    method: String,
    mut result: Value,
    reply: tokio::sync::mpsc::Sender<helix_lsp::Result<Value>>,
) {
    let plugins: Vec<String> = FILTERS
        .lock()
        .iter()
        .filter(|(_, methods)| methods.contains(&method))
        .map(|(plugin, _)| plugin.clone())
        .collect();
    tokio::spawn(async move {
        for plugin in plugins {
            let payload = json!({ "server": server, "result": result }).to_string();
            match serve(super::editor_call(&plugin, "lsp-filter", &method, payload)).await {
                Ok(changed) => match serde_json::from_str(&changed) {
                    Ok(changed) => result = changed,
                    Err(err) => log::warn!("{plugin}: lsp-filter reply for {method}: {err}"),
                },
                Err(err) => log::warn!("{plugin}: lsp-filter for {method}: {err}"),
            }
        }
        let _ = reply.send(Ok(result)).await;
    });
}

/// Serve the plugin language server `name` over `stream` (from
/// `helix_lsp`).
fn start_server(name: String, stream: tokio::io::DuplexStream) {
    tokio::spawn(run(name, stream));
}

async fn run(name: String, stream: tokio::io::DuplexStream) {
    let (read, mut write) = tokio::io::split(stream);
    let mut read = BufReader::new(read);
    let (out, mut outgoing) = mpsc::unbounded_channel::<Value>();
    // One writer, so replies don't interleave.
    tokio::spawn(async move {
        while let Some(message) = outgoing.recv().await {
            let body = message.to_string();
            let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
            if write.write_all(frame.as_bytes()).await.is_err() || write.flush().await.is_err() {
                break;
            }
        }
    });
    while let Some(message) = read_message(&mut read).await {
        let (Some(id), Some(method)) = (message.get("id").cloned(), message.get("method")) else {
            // A notification (the exit one ends it) or a reply to us.
            if message.get("method").and_then(Value::as_str) == Some("exit") {
                break;
            }
            continue;
        };
        let method = method.as_str().unwrap_or_default().to_string();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method.as_str() {
            "initialize" => {
                let capabilities = SERVERS
                    .lock()
                    .get(&name)
                    .map(|(_, capabilities)| capabilities.clone())
                    .unwrap_or_else(|| json!({}));
                let result =
                    json!({ "capabilities": capabilities, "serverInfo": { "name": name } });
                let _ = out.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
            "shutdown" => {
                let _ = out.send(json!({ "jsonrpc": "2.0", "id": id, "result": null }));
            }
            _ => {
                let plugin = SERVERS.lock().get(&name).map(|(plugin, _)| plugin.clone());
                let out = out.clone();
                tokio::spawn(async move {
                    let reply = match plugin {
                        Some(plugin) => serve(super::editor_call(
                            &plugin,
                            "lsp",
                            &method,
                            params.to_string(),
                        ))
                        .await
                        .and_then(|reply| {
                            serde_json::from_str::<Value>(&reply).map_err(|err| err.to_string())
                        }),
                        None => Err("the plugin is gone".into()),
                    };
                    let message = match reply {
                        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        Err(err) => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32603, "message": err },
                        }),
                    };
                    let _ = out.send(message);
                });
            }
        }
    }
}

/// The next LSP message (`Content-Length` framed); `None` at the end.
async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(read: &mut R) -> Option<Value> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if read.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header.strip_prefix("Content-Length:") {
            length = value.trim().parse::<usize>().ok();
        }
    }
    // A sane bound: the editor sends whole documents, but not gigabytes.
    let length = length.filter(|length| *length <= 256 << 20)?;
    let mut body = vec![0; length];
    read.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}
