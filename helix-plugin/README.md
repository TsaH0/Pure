# Pure plugins

Plugins are [WebAssembly components](https://component-model.bytecodealliance.org)
run in a sandbox by [Wasmtime](https://wasmtime.dev). The API is defined in
[`wit/v0.2/helix-plugin.wit`](wit/v0.2/helix-plugin.wit); any language that can build a
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

Ready to use: copy an example's `plugin.toml` and `plugin.wasm` into
`~/.config/helix/plugins/<name>/`.

| Example | Commands | `plugins.toml` |
| --- | --- | --- |
| [`text-tools`](examples/text-tools) | `upper`, `lower`, `sort-lines`, `count` | – |
| [`yazi`](examples/yazi) | `open`: pick files with [yazi](https://yazi-rs.github.io) | `[yazi]` `allow-process = ["yazi"]` |
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
  `:plugin-reload`) and the editor keeps going.

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
api = "0.2"
description = "Says hello"
permissions = ["read-buffers", "edit-buffers"]
```

Build with `cargo build --release --target wasm32-wasip2` and copy
`target/wasm32-wasip2/release/my_plugin.wasm` to
`~/.config/helix/plugins/my-plugin/plugin.wasm` next to the manifest.

See [`examples/text-tools`](examples/text-tools) for a complete plugin and
[`tests/plugins/testplugin`](tests/plugins/testplugin) for every API function.

## Development

- `cargo test -p helix-plugin` tests the host with real plugins.
- `cargo test -p helix-term --features integration --test integration -- plugins`
  tests plugins in a running editor.
- `./build-plugins.sh` rebuilds the 0.1 and 0.2 test plugins (`tests/fixtures`)
  and the example after changing guest code or the SDK. The committed
  `text-tools-v0.1.wasm` is a frozen compatibility fixture.


## API versions and SDK callbacks

Pure accepts `api = "0.1"` and `api = "0.2"`. Existing 0.1 components keep
loading without rebuilding. SDK 0.2 keeps the same `Plugin` trait methods and
buffer API; `text-tools` builds unchanged with `api = "0.2"`. To rebuild an
older component with the 0.1 ABI, enable the SDK's `api-v0-1` feature:

```toml
helix-plugin-api = { path = "/path/to/helix/helix-plugin/sdk", features = ["api-v0-1"] }
```

SDK 0.2 also exports callback bindings automatically. `editor::spawn` and
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

UI (`run-interactive`, `pick`, `run-command`) is restricted to plugin commands
and callbacks, so events cannot interrupt typing. API 0.1's
WIT remains at [`wit/v0.1/`](wit/v0.1/); existing versioned interfaces are never
extended in place.

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

- `editor::spawn(argv, SpawnOptions { cwd, stdin, timeout_ms }, callback)` runs
  a program in the background. The callback gets its exit code (`None` if it
  was killed or timed out), stdout and stderr (lossy UTF-8, at most 16 MiB
  each). The default time limit is 30 s, at most 10 min. `editor::kill(id)`
  stops it; at most 16 programs run at once per plugin.
- `editor::run_interactive(argv, InteractiveOptions { cwd, chooser_file,
  wait_for_key }, callback)` hands the terminal to a full-screen program and
  calls back when it exits. With `chooser_file`, an argument containing
  `{chooser}` gets the path of a fresh private temporary file, and the
  callback's `stdout` is what the program wrote there. With `wait_for_key`,
  the program runs on a cleared screen and waits for a key afterwards (like
  `space z`). Only from commands and callbacks.

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
and `.with_file_preview(path, lines)` build richer items. Text previews are
not shown yet. Only from commands and callbacks.

`editor::pick_live(picker, query, callback)` is a live picker: what the user
types goes to `query(id, text)` (debounced) instead of filtering, and the
items it returns are shown. To answer later, e.g. from a program, return
`Ok(None)` and call `editor::set_items(id, items)` (see `examples/finder`,
`grep`).

## Prompts and popups

- `editor::prompt(message, initial, completions, callback)` asks for a line
  of text; the callback gets `Some(text)`, or `None` on Esc. `completions`
  are offered (fuzzy matched) while typing.
- `editor::popup(markdown)` shows markdown at the cursor until the next key.

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

Paths are the real ones (as `document-path` returns them). Plugins see the
workspace from when they were loaded: after `:cd`, `:plugin-reload`. Also
available: `editor::cwd()`, `editor::config_dir()` and
`editor::document_modified(doc)`.

## Events

Subscribe in `init`'s `Registration { events, .. }` and implement the
`Plugin` trait's handler: `on_document_open`, `on_document_change`
(debounced), `on_mode_switch`, and with API 0.2 `on_document_save`,
`on_document_close` (the document is gone; only its id is known) and
`on_selection_change` (debounced). Handlers get 250 ms and can't open UI or
run commands.
