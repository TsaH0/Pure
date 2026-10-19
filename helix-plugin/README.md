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
```

Try the example: copy [`examples/text-tools`](examples/text-tools) (its
`plugin.toml` and `plugin.wasm`) into `~/.config/helix/plugins/text-tools/`.

## Sandbox

- A plugin only gets the host functions for the **permissions** its manifest
  lists; anything else fails with "permission denied":
  - `read-buffers`: document text, paths, languages, selections
  - `edit-buffers`: change text and selections
  - `open-files`: open files
  - `run-commands`: run editor commands (from plugin commands and callbacks)
  - `process`: request external programs, with a separate explicit user grant
- No file system, network, environment variables or standard streams (WASI is
  linked with nothing granted).
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

Background process execution and callback plumbing are available. Terminal
hand-over and picker display follow in their separate milestones; those
imports still return an explicit unsupported error until implemented. UI is restricted to
plugin commands and callbacks, so events cannot interrupt typing. API 0.1's
WIT remains at [`wit/v0.1/`](wit/v0.1/); existing versioned interfaces are never
extended in place.


## Running programs: permission and grant

A program runs outside WebAssembly with the user's rights. A plugin must list
`"process"` in its `plugin.toml` permissions **and** receive a user grant in
`~/.config/helix/plugins.toml`:

```toml
[yazi]
allow-process = ["yazi"]

[git]
allow-process = ["git"]

[trusted-tools]
allow-process = true
```

Missing grants, `false`, and empty lists deny execution. Lists match the exact
program argument (`argv[0]`), looked up in `PATH`; granting `rg` does not also
grant `/tmp/rg`. Arguments are passed directly, without a shell. Shell execution
requires explicitly granting the shell program. Reload plugins after changing
grants. Invalid grant values prevent that plugin loading, while other plugins
continue to load. Grants are host settings and are not forwarded to `init`.

`:plugin-list` and the general `--health` report `process: granted`,
`process: not granted`, or `process: only rg, fd`. No first-use prompt is
implemented; explicit configuration is the current grant mechanism. Background execution is available; terminal hand-over follows in M1.3.


### Background programs

```rust,ignore
use helix_plugin_api::{editor, SpawnOptions};

let request = editor::spawn(&["git", "status", "--short"], SpawnOptions::default(), |result| {
    let output = result?;
    if output.exit_code != Some(0) {
        return Err(output.stderr);
    }
    editor::set_status(&output.stdout);
    Ok(())
})?;
// To cancel later: editor::kill(request). The completion still arrives.
```

`SpawnOptions` accepts `cwd`, `stdin` (written and then closed), and
`timeout_ms` (30 seconds by default, clamped to at most ten minutes).
There is no implicit shell. `Output` contains an optional exit code and lossy
UTF-8 stdout/stderr, capped at 16 MiB each; truncation is noted in stderr.
Signals, cancellation and timeouts have no exit code. A failed spawn returns
an error to the callback; nonzero program exits are ordinary outputs.

Each plugin may have at most 16 outstanding process requests. Reload or
disabling cancels its processes, and late results cannot reach the next
instance. Callbacks get a fresh editor snapshot and the normal command time
limit; their edits and editor commands apply through the job queue. `spawn`
also works in event handlers; UI operations remain restricted.

When passing flags to a plugin through `:plugin-run`, use the existing `--`
separator before its arguments (for example `:plugin-run my-plugin/run -- -v`).
This preserves the editor's existing command parser and quoting behavior.
