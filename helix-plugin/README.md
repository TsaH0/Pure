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
| [`oil`](examples/oil) | `open` (`-`): edit directories as buffers like oil.nvim; rename, move, copy, create and delete with `:w` | `[oil]` `allow-files = "write"`, `allow-dirs = ["~"]` |
| [`lint`](examples/lint) | diagnostics for trailing space, long lines, mixed indentation, conflict markers | `[lint]` `max-line = 100` |
| [`emoji`](examples/emoji) | completion source: `:smi` completes to 😄 | – |
| [`pomodoro`](examples/pomodoro) | `start`, `stop`: a timer in the statusline | `[pomodoro]` `work = 25`, `rest = 5` |
| [`yank-history`](examples/yank-history) | `list`: pick an earlier yank to paste | – |
| [`api-demo`](examples/api-demo) | popup callbacks, live queries, streaming, positioned opens | `[api-demo]` `allow-process = ["sh"]` for streaming |
| [`js-upper`](examples/js-upper) | `upper` (written in JavaScript) | – |
| [`markdown`](examples/markdown) | `toggle` (`space M`); renders Markdown in the buffer like render-markdown.nvim | – |
| [`todos`](examples/todos) | `list`; highlights TODO / FIXME / NOTE, signs, end-of-line notes, a statusline count | – |
| [`finder`](examples/finder) | `files` (rg), `grep` (live, rg), `branches` (git) | `[finder]` `allow-process = ["rg", "git"]` |

```toml
# config.toml
[keys.normal.space]
f = ":plugin-run finder/files"
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
  - `registers`: read and write registers (API 0.3; the clipboard can only
    be written).
- No network, environment variables or standard streams; no files except
  the plugin's own directory and, if granted, the workspace and the
  `allow-dirs` directories.
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

Pure accepts `api = "0.1"` to `api = "0.4"` side by side. Existing
components load without rebuilding. SDK 0.4 generates the 0.4 ABI by
default; set `api = "0.4"` in the plugin manifest. To rebuild an older
component, select exactly one legacy feature and keep its manifest API:

```toml
# For api = "0.3" (api-v0-2 for "0.2", api-v0-1 for "0.1"):
helix-plugin-api = { path = "/path/to/helix/helix-plugin/sdk", features = ["api-v0-3"] }
```

0.4 differs from 0.3 only in `files`: 0.3's `create_path`, `rename_path`
and `delete_path` became `files::apply`, which reports results (and can copy
and trash); plugins export one more callback (the SDK does that).

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
WIT remains at [`wit/v0.1/`](wit/v0.1/), 0.2 at [`wit/v0.2/`](wit/v0.2/)
and 0.3 at [`wit/v0.3/`](wit/v0.3/). A released version may gain host
functions (imports: old components don't use them), never exports.

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
  `!`). Chooser files must be regular files; reads are capped at 16 MiB
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

Users can grant more directories than the workspace (absolute, `~` is the
home directory); `files::roots()` lists what the plugin got:

```toml
[oil]
allow-files = "write"
allow-dirs = ["~", "/tmp"]
```

### Changing files through the editor (API 0.4)

`std::fs` changes files behind the editor's back. `helix_plugin_api::files`
asks the editor instead, like `:move` does, so buffers follow renamed files
and language servers hear about it (and can, say, update imports):

```rust,ignore
use helix_plugin_api::files::{self, CreateOp, FileOp, MoveOp};

let ops = [
    FileOp::Create(CreateOp { path: "/work/src/new.rs".into(), directory: false }),
    FileOp::Rename(MoveOp { from: "/work/a.rs".into(), to: "/work/src/a.rs".into() }),
    FileOp::Copy(MoveOp { from: "/work/assets".into(), to: "/work/assets-old".into() }),
    FileOp::Trash("/work/old".into()), // or Delete: final
];
files::apply(&ops, |results| {
    // One result per operation, in order; after a failure: "skipped".
    for result in results {
        result?;
    }
    editor::set_status("done");
    Ok(())
})?;
```

They need `filesystem` and `allow-files = "write"` and take absolute paths
inside `files::roots()` (no `..`, and no symlink leading out). They run in
order after the call, off the editor's thread for copies and the trash;
none overwrites an existing path. `Trash` uses the system's trash and fails
rather than deleting if it can't. (API 0.3 has fire-and-forget
`create_path`, `rename_path` and `delete_path` instead.)

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
- Overlays (`set_overlays`, `decorations::overlay`) draw other characters
  *over* the text (one over each char), e.g. `◉` over `#`; the text itself
  is unchanged. Hiding characters (conceal) isn't possible.
- Signs are one character on a line, shown in the `plugins` gutter, which
  users add to `editor.gutters`.
- The statusline text shows in the `plugins` element of
  `editor.statusline` (one line, at most 200 chars per segment).
- At most 100,000 decorations per layer. Reloading plugins clears them.

See `examples/todos`.

## Diagnostics (API 0.3)

Linters report problems with `helix_plugin_api::diagnostics`. They show
like a language server's: gutter, inline and end-of-line text, `]d` / `[d`,
the diagnostics pickers and the statusline counts.

```rust,ignore
use helix_plugin_api::diagnostics::{self, diagnostic, Severity};

let mut d = diagnostic(from, to, Severity::Warning, "unused variable");
d.code = Some("W001".into());
diagnostics::set_diagnostics(doc, &[d])?; // replaces this plugin's, [] clears
```

Positions are char offsets; the diagnostics follow edits until set again.
Each plugin has one set per document (`source` defaults to its name);
reloading plugins removes them. See `examples/lint`.

## Completion sources (API 0.3)

Subscribe to `EventKind::Completion` and the editor asks the plugin for
completions while typing, next to language servers and words:

```rust,ignore
fn on_completion(id: u32, doc: DocumentId, position: u32) {
    let mut item = completion::item("println!");
    item.kind = Some("snippet".into());
    item.documentation = Some("Print a line".into());
    let _ = completion::complete(id, &[item]);
}
```

Answer once per request, now or from a later callback (e.g. after a
program ran); the menu waits up to 1 s, and a new request replaces an
unanswered one. Items replace the word before the cursor, or the text from
`replace-from`; the editor filters them by what was typed. See
`examples/emoji`.

## Timers (API 0.3)

`editor::set_timer(ms, callback)` runs `callback` once, later; set another
from it to repeat, `editor::cancel_timer(id)` stops one. At most 16 run at
once. Nothing runs (or wakes the editor) while no timer is set, so prefer
a timer to `on_editor_idle` only when the plugin needs a clock. The
callback is a callback: it can open UI and run commands. See
`examples/pomodoro`.

## Buffers (API 0.3)

- `editor::open_scratch(text, language, action)` opens a new scratch buffer
  (no file) with `text`, e.g. command output or a REPL transcript.
- `editor::open_buffer(name, text, language, action)` opens a buffer the
  plugin **owns**, shown as `name` (e.g. `oil:///work/`). `:write` on it
  calls the plugin's `on_buffer_write(doc)` instead of writing a file (as a
  command: prompts and pickers work); call `editor::mark_saved(doc)` once
  it's applied, else it stays modified and `:q` warns. Opening the same
  name again shows that buffer. After it opens, the owner's
  `on_document_open(doc)` runs (without subscribing), so it learns the id;
  `editor::buffer_name(doc)` tells its own buffers apart.
- `[keys.buffer.<mode>]` in `plugin.toml` binds keys only in the plugin's
  own buffers, where they take precedence (like Neovim's buffer-local
  mappings); elsewhere they do nothing.

`examples/oil` uses all of them: a directory as an editable buffer.

## Registers (API 0.3)

With the `registers` permission, `editor::register(name)` reads a register
(one value per selection) and `editor::set_register(name, values)` writes
one. The clipboard registers (`+`, `*`) can be written, not read. See
`examples/yank-history`.

## Default key bindings

A plugin can suggest bindings in `plugin.toml`, in the same shape as
`config.toml`'s `[keys]`; values are the plugin's own command names:

```toml
[keys.normal.space]
T = "list"          # runs :todos/list
```

They only fill keys nothing else binds (neither the user's config nor Pure's
defaults), so they never replace a binding. `[keys.buffer.<mode>]` is the
exception: those apply only in the plugin's own buffers (see
[Buffers](#buffers-api-03)). Users turn them off with
`keys = false` in the plugin's `plugins.toml` table. Plugin commands also
run as `:<plugin>/<command>` and appear in the command palette.

## Background tasks (API 0.3)

Heavy work (parsing a big file, building an index) would freeze the editor
in a command or event handler, which have 2 s and 250 ms. Run it in the
background instead:

```rust,ignore
// In a command or event handler: send the text to work on.
editor::run_background("outline", editor::text(doc)?, move |result| {
    let outline = result?;            // what run_background returned
    editor::set_status(&outline);     // back in the main instance
    Ok(())
})?;

// In the plugin:
impl Plugin for MyPlugin {
    fn run_background(task: &str, input: String) -> Result<String, String> {
        match task {
            "outline" => Ok(compute_outline(&input)),
            _ => Err(format!("unknown task {task}")),
        }
    }
    // ...
}
```

`run_background` runs in a *second instance* of the plugin, on its own
thread, with up to 30 s. It has its own memory (statics are not shared with
the main instance) and no editor access: documents read as missing and
anything it asks the editor to do is dropped. Pass everything it needs in
`input` and return the result as a string. At most 4 tasks run at once per
plugin; a task that crashes or runs out of time gives an error result and
the next task gets a fresh instance (the plugin stays enabled).

## Distributing a plugin

Start from the template:

```sh
cargo generate --git https://github.com/TsaH0/helix-editor-custom helix-plugin/template
```

Build it (`cargo build --release --target wasm32-wasip2`), copy the `.wasm`
to `plugin.wasm` next to `plugin.toml`, and commit both. Users install it
with `:plugin-install <git URL>`; nothing is built on their machines.
`plugins.lock` pins the installed component's SHA-256.

## Other languages

Any language that builds a component for `wit/v0.3` works; the WIT is the
whole contract. A plugin must export every interface of the world (`plugin`,
`callbacks`, `events`, `background`); empty handlers are fine.

- **JavaScript** (verified): see [`examples/js-upper`](examples/js-upper),
  built with `npm install && npm run build` (`jco componentize ... --disable
  http --disable fetch-event`). The component bundles a JS engine: 12 MB, a
  cold first load compiles for ~55 s in a debug build of Pure (seconds in a
  release build) and 0.4 s from the cache after that, on Pure's background
  loading thread.
- **Python** (untested here): `componentize-py --wit-path wit/v0.3 --world
  helix-plugin componentize app -o plugin.wasm`.
- **Go** (untested here): TinyGo 0.34+ with `wit-bindgen-go generate
  wit/v0.3`, then `tinygo build -target=wasip2 --wit-package wit/v0.3
  --wit-world helix-plugin -o plugin.wasm`.
