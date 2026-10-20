# Pure plugins

Plugins are [WebAssembly components](https://component-model.bytecodealliance.org)
run in a sandbox by [Wasmtime](https://wasmtime.dev). The API is defined in
[`wit/v0.3/helix-plugin.wit`](wit/v0.3/helix-plugin.wit); any language that can build a
component for it works. Rust has an SDK in [`sdk/`](sdk).

## Using plugins

Each plugin is a directory in `~/.config/helix/plugins/` with a `plugin.toml`
and the component:

```
~/.config/helix/plugins/
└── text-tools/
    ├── plugin.toml
    └── plugin.wasm
```

Plugins are loaded at startup; `:plugin-reload` reloads them. Their commands run
with `:plugin-run <plugin>/<command> [args...]` (with completion) and can be
bound like any typable command:

```toml
# ~/.config/helix/config.toml
[keys.normal.space]
u = ":plugin-run text-tools/upper"
```

`:plugin-list` shows the loaded plugins, their permissions and commands, and
why a plugin was disabled.

Settings go in `~/.config/helix/plugins.toml`, one table per plugin. Everything
except `enabled` and `allow-process` is passed to the plugin's `init` as JSON:

```toml
[text-tools]
enabled = false

[my-plugin]
width = 80
allow-process = ["rg", "git"]   # see "Running programs"
allow-files = "read"            # see "Files"
```

### Example plugins

Pure ships the API; examples are optional and are never installed automatically.
Copy an example's `plugin.toml` and `plugin.wasm` into
`~/.config/helix/plugins/<name>/`.

| Example | Commands | `plugins.toml` |
| --- | --- | --- |
| [`text-tools`](examples/text-tools) | `upper`, `lower`, `sort-lines`, `count` | – |
| [`yazi`](examples/yazi) | `open`: pick files with [yazi](https://yazi-rs.github.io) | `[yazi]` `allow-process = ["yazi"]` |
| [`api-demo`](examples/api-demo) | popup callbacks, live queries, streaming, positioned opens | `[api-demo]` `allow-process = ["sh"]` for streaming |
| [`todos`](examples/todos) | `list`; highlights TODO / FIXME / NOTE, signs, end-of-line notes, a statusline count | – |
| [`finder`](examples/finder) | `files` (rg), `grep` (live, rg), `branches` (git) | `[finder]` `allow-process = ["rg", "git"]` |

```toml
# config.toml
[keys.normal.space]
e = ":plugin-run yazi/open"
"/" = ":plugin-run finder/grep"
```

## Sandbox

- A plugin only gets the host functions for the **permissions** its manifest
  lists; anything else fails with "permission denied":
  - `read-buffers`: document text, paths, languages, selections
  - `edit-buffers`: change text and selections
  - `open-files`: open files
  - `run-commands`: run editor commands (only from the plugin's own commands
    and callbacks)
  - `process`: run programs, see [Running programs](#running-programs). This
    one also needs the user's `allow-process` grant.
  - `filesystem`: read (or write) the workspace, see [Files](#files). This one
    also needs the user's `allow-files` grant.
- No network, environment variables or standard streams; no files except
  the plugin's own directory and, if granted, the workspace.
- Each call has a **time limit** (2 s for commands, 250 ms for event handlers,
  5 s for `init`) and plugins get at most **256 MiB** of memory. A plugin that
  runs out of time, out of memory or crashes is **disabled** (until
  `:plugin-reload`) and the editor keeps going. Time limits also cover
  suspended WASI calls, such as sleeping or waiting to open a FIFO.

## How calls work

When Pure calls a plugin (a command or an event) the plugin sees a snapshot
of the editor: open documents, their text, paths, languages and selections, the
focused document and the mode. Changes the plugin makes (edits, selections,
messages, opening files, commands) are collected and applied after the call
returns, in order. All edits of one call to a document are one undo step. Edits
are checked: they must be in bounds and not overlap, otherwise the call fails
and nothing is applied. Reads after an edit in the same call see the edit.

Positions are **char** offsets (not bytes), like everywhere in Pure. In
Pure a cursor is a one-char selection.

Events (`document-open`, `document-change`, `mode-switch`) are delivered
shortly after they happen; several changes to a document in a row (typing)
become one `document-change`. Edits a plugin makes don't trigger change events,
so a plugin reacting to changes can't loop forever.

## Writing a plugin in Rust

```sh
rustup target add wasm32-wasip2
cargo new --lib my-plugin
```

`Cargo.toml`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
helix-plugin-api = { path = "/path/to/helix/helix-plugin/sdk" }
```

`src/lib.rs`:

```rust
use helix_plugin_api::{editor, register_plugin, CommandInfo, Edit, Plugin, Registration};

struct MyPlugin;

impl Plugin for MyPlugin {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo {
                name: "hello".into(),
                doc: "Insert a greeting at the cursor".into(),
            }],
            events: vec![],
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "hello" => {
                let doc = editor::current_document().ok_or("no document")?;
                let cursor = editor::selections(doc)?[0].head;
                let text = format!("hello {}", args.join(" "));
                editor::apply_edits(doc, &[Edit { from: cursor, to: cursor, text }])
            }
            _ => Err(format!("unknown command {name}")),
        }
    }
}

register_plugin!(MyPlugin);
```

`plugin.toml`:

```toml
name = "my-plugin"
version = "0.1.0"
api = "0.3"
description = "Says hello"
permissions = ["read-buffers", "edit-buffers"]
```

Build with `cargo build --release --target wasm32-wasip2` and copy
`target/wasm32-wasip2/release/my_plugin.wasm` to
`~/.config/helix/plugins/my-plugin/plugin.wasm` next to the manifest.

See [`examples/text-tools`](examples/text-tools) for a complete plugin and
[`examples/api-demo`](examples/api-demo) for API 0.3 callback examples.
The versioned WIT files are the complete API reference.

## Development

- `cargo test -p helix-plugin` tests the host with real plugins.
- `cargo test -p helix-term --features integration --test integration -- plugins`
  tests plugins in a running editor.
- `./build-plugins.sh` rebuilds the 0.1, 0.2 and 0.3 test plugins (`tests/fixtures`)
  and every example after changing guest code or the SDK. The committed
  `text-tools-v0.1.wasm`, `text-tools-v0.2.wasm`, and
  `testplugin2-v0.2.wasm` are frozen compatibility fixtures and are never
  overwritten by the script.


## API versions and SDK callbacks

Pure accepts `api = "0.1"`, `api = "0.2"` and `api = "0.3"` side by side.
Existing 0.1/0.2 components load without rebuilding. SDK 0.3 generates the
0.3 ABI by default; set `api = "0.3"` in the plugin manifest. To rebuild an
older component, select exactly one legacy feature and keep its manifest API:

```toml
# For api = "0.2" (use api-v0-1 for api = "0.1"):
helix-plugin-api = { path = "/path/to/helix/helix-plugin/sdk", features = ["api-v0-2"] }
```

The SDK exports callback bindings automatically. `editor::spawn` and
`editor::run_interactive` accept a one-shot closure receiving `Result<Output,
String>`; `editor::pick` accepts a picker and a closure receiving `Choice`
(items, original indices, and an optional split action; no action means
cancelled). Each returns a request id, which `editor::kill` can use for a
background program. A callback may start another request. Unknown/duplicate
results are ignored; callback errors become editor errors. Failed commands
discard the closures they registered.

```rust,ignore
use helix_plugin_api::{editor, Item, Picker, SpawnOptions};

editor::spawn(&["rg", "--files"], SpawnOptions::default(), |output| {
    let items = output?.stdout.lines().map(Item::text).collect();
    editor::pick(Picker::new("Files", items), |choice| {
        for item in choice.items {
            editor::open_file(&item.data)?;
        }
        Ok(())
    })?;
    Ok(())
})?;
```

UI (`run-interactive`, `pick`, popups, `run-command`) is restricted to plugin commands
and callbacks, so events cannot interrupt typing. API 0.1's
WIT remains at [`wit/v0.1/`](wit/v0.1/) and 0.2 at
[`wit/v0.2/`](wit/v0.2/); released interfaces are never extended in place.

## Running programs

Programs run with the user's rights, outside the sandbox, so two things are
needed: the plugin lists the `process` permission in its `plugin.toml`, and
the user grants it in `plugins.toml`:

```toml
[my-plugin]
allow-process = true             # any program
# allow-process = ["rg", "fd"]   # only these (compared with argv[0] as given)
```

Without the grant `spawn` and `run-interactive` fail with a message saying
what to add; `:plugin-list` shows the grant. Programs are started directly
(no shell), in the editor's working directory unless `cwd` is given.

**`allow-process` is a trust decision, not a sandbox.** A program on the
list can start other programs: for example, `rg --pre` runs a preprocessor,
and `git -c alias.x=!sh x` invokes a shell. Shells, scripts and program options
can therefore give a plugin arbitrary execution with your user account's
rights, including access to files and the network. The list checks only the
requested `argv[0]`, not what that program subsequently does. Grant it only
to plugins you trust; Pure does not ask again for each program.

A workspace write grant is also powerful: a plugin can change a project's
git hooks, which may run later when you use git. Separately, the legacy
`run-commands` permission allows editor commands such as `:sh` and `:write`;
it should also be granted only to trusted plugins.

- `editor::spawn(argv, SpawnOptions { cwd, stdin, timeout_ms }, callback)` runs
  a program in the background. The callback gets its exit code (`None` if it
  was killed or timed out), stdout and stderr (lossy UTF-8, at most 16 MiB
  each). The default time limit is 30 s, at most 10 min. `editor::kill(id)`
  stops it; at most 16 programs run at once per plugin.
- API 0.3 `editor::spawn_stream(argv, options, on_output, on_done)` delivers
  complete stdout lines (including their newlines) in bounded batches before
  completion. `on_done` receives only the unfinished stdout tail, the full
  stderr and the exit code, after all delivered batches. Do not append the
  batches again from the final result. The 16 MiB stdout cap is cumulative,
  including already delivered bytes. Backpressure bounds queued data; no
  periodic flush timer is used. Output callbacks may update pickers or start
  other requests. Killing a process still delivers its final callback.
- `editor::run_interactive(argv, InteractiveOptions { cwd, chooser_file,
  wait_for_key }, callback)` hands the terminal to a full-screen program and
  calls back when it exits. With `chooser_file`, an argument containing
  `{chooser}` gets the path of a fresh private temporary file, and the
  callback's `stdout` is what the program wrote there. With `wait_for_key`,
  the program runs on a cleared screen and waits for a key afterwards (like
  `space z`). Chooser files must be regular files; reads are capped at 16 MiB
  with a truncation notice in stderr. Pure refuses replacement FIFOs on Unix
  without waiting for a writer. Only from commands and callbacks.

A file chooser, e.g. with yazi:

```rust,ignore
editor::run_interactive(
    &["yazi", "--chooser-file={chooser}"],
    InteractiveOptions { chooser_file: true, ..Default::default() },
    |output| {
        for path in output?.stdout.lines().filter(|l| !l.is_empty()) {
            editor::open_file(path)?;
        }
        Ok(())
    },
)?;
```

## Pickers

`editor::pick(Picker, callback)` shows Pure's picker (fuzzy matching, preview,
`C-s`/`C-v` splits) with the plugin's items: up to 8 columns and a million
items. `Item::text(s)` is a one-column item whose data is `s`; items can also
carry a file preview (path and lines to highlight). The callback gets a
`Choice`: the chosen items and their indices, and the split (`None` when the
picker was closed without choosing). `editor::push_items(id, items)` adds items to an
open picker, e.g. as a program's output arrives; `Item::new(columns, data)`
and `.with_file_preview(path, lines)` / `.with_text_preview(text)` build
richer items. Only from commands and callbacks. The million-item limit covers
initial items plus every push; `set_items` resets the count. A rejected push
leaves the picker unchanged.

API 0.3 `editor::pick_live(picker, query, callback)` calls
`query(id, generation, text)` after debounce. Return `Ok(Some(items))` for
immediate results, or `Ok(None)` to answer asynchronously. Capture the supplied
generation and use `editor::set_items_for_query(id, generation, items)` or
`editor::push_items_for_query(id, generation, items)` for delayed results.
These helpers discard obsolete generations and closed pickers. The host also
invalidates results immediately on input, before debounce, and drops old
queued batches; a plugin-local counter alone cannot cover that gap. Static
pickers continue to use `set_items` and `push_items` without a generation.
API 0.2 keeps `query(id, text)` and its original untagged update functions.

See [`finder`](examples/finder): `files` opens an empty picker before streaming
`rg --files` into it, and `grep` tags each query's asynchronous results. Closing
either picker kills its producer. Finder opens files with the native API and
needs only `open-files` and `process`, with the user's `allow-process` grant.

## Opening a file at a position

API 0.3 `editor::open_file_at(path, action, line, column)` requires
`open-files`. `action` is `OpenAction::Replace`, `Hsplit` or `Vsplit`.
Coordinates are optional, **one-based** line and character column (not UTF-8
byte columns); missing coordinates use the start, and out-of-range coordinates
are clamped. `editor::open_file(path)` remains available in every API version.

```rust,ignore
editor::open_file_at("src/main.rs", OpenAction::Vsplit, Some(12), Some(3))?;
```

## Prompts and popups

- `editor::prompt(message, initial, completions, callback)` asks for a line
  of text; the callback gets `Some(text)`, or `None` on Esc. `completions`
  are offered (fuzzy matched) while typing.
- API 0.3 `editor::popup(markdown)` shows markdown at the cursor and returns
  an owned id. `editor::close_popup(id)` closes that popup only.
  `editor::popup_with_options(markdown, PopupOptions { centered: true },
  on_close)` centers it and calls `on_close(id)` once when it closes by id,
  Esc, or the next key. Esc is consumed; other keys continue to the editor.
  Multiple owned popups may coexist. Foreign or completed ids are ignored.
  API 0.2 retains `popup(markdown) -> Result<(), String>` and its original
  replacement behavior.

Both only from commands and callbacks.

## Files

Each plugin has its own directory, `editor::data_dir()`
(`~/.local/share/helix/plugins/<name>/`), and can read and write it with
`std::fs` without any permission. The workspace (`editor::workspace_root()`)
needs the `filesystem` permission and the user's grant:

```toml
[my-plugin]
allow-files = "read"    # or "write"
```

Paths are the real ones (as `document-path` returns them). When `:cd` moves
to another workspace, plugins with `allow-files` are reloaded to see it. Also
available: `editor::cwd()`, `editor::config_dir()` and
`editor::document_modified(doc)`.

## Events

Subscribe in `init`'s `Registration { events, .. }` and implement the
`Plugin` trait's handler: `on_document_open`, `on_document_change`
(debounced), `on_mode_switch`, and with API 0.2 `on_document_save`,
`on_document_close` (the document is gone; only its id is known) and
`on_selection_change` (debounced). Handlers get 250 ms and can't open UI or
run commands.

API 0.3 also provides `on_post_command(name)`, `on_focus_gained()`,
`on_focus_lost()` and `on_editor_idle()`, with matching `EventKind` variants.
Post-command reports completed native command dispatch; plugin-applied commands
are suppressed to prevent feedback loops. Focus events work even when a picker
or popup is open. Idle uses the editor's existing idle timer and fires once
until input re-arms it. Unsubscribed event hooks return after an atomic mask
check; subscribing adds no polling timer or periodic idle wake-up.

## Decorations (API 0.3)

`helix_plugin_api::decorations` changes how documents look. Each kind is set
per layer (a name the plugin picks); setting a layer replaces it and an
empty list clears it. Positions are char offsets into the text the call sees
and follow the user's edits until the plugin sets the layer again. They work
from commands, callbacks and event handlers, so a plugin can redecorate on
`on_document_change`. No permission is needed (decorations only change what
is shown); reading the text to compute them needs `read-buffers`.

```rust,ignore
use helix_plugin_api::decorations::{self, annotation, highlight, sign};

decorations::set_highlights(doc, "words", &[highlight(4, 8, "warning")])?;
decorations::set_annotations(doc, "notes", &[annotation(line_end, "  ◂ todo", "comment")])?;
decorations::set_signs(doc, "lines", &[sign(0, "●", "warning")])?;
decorations::set_statusline("count", "3 todos")?;
```

- Highlights use theme scopes (`warning`, `diff.plus`, ...). Overlapping
  ones in one layer: the first wins.
- Virtual text goes before the char at its position; the offset of a line's
  line break puts it at the end of the line. No line breaks in the text.
- Signs are one character on a line, shown in the `plugins` gutter, which
  users add to `editor.gutters`.
- The statusline text shows in the `plugins` element of
  `editor.statusline` (one line, at most 200 chars per segment).
- At most 100,000 decorations per layer. Reloading plugins clears them.

See `examples/todos`.

## Default key bindings

A plugin can suggest bindings in `plugin.toml`, in the same shape as
`config.toml`'s `[keys]`; values are the plugin's own command names:

```toml
[keys.normal.space]
T = "list"          # runs :todos/list
```

They only fill keys nothing else binds (neither the user's config nor Pure's
defaults), so they never replace a binding. Users turn them off with
`keys = false` in the plugin's `plugins.toml` table. Plugin commands also
run as `:<plugin>/<command>` and appear in the command palette.
