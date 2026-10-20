# AGENTS.md: building plugins for Pure (a Helix fork)

You are writing a plugin for **Pure**, a fork of the Helix editor (binary
`pure`, source <https://github.com/TsaH0/Pure>). Everything in
this file comes from that repository. When in doubt, the WIT file is the
contract: `helix-plugin/wit/v0.4/helix-plugin.wit`. The long-form guide is
`helix-plugin/README.md`. Do not invent host functions. If something is not
listed here or in the WIT, it does not exist.

Pure keeps Helix's paths: config is `~/.config/helix/` (`$XDG_CONFIG_HOME/helix`),
data is `~/.local/share/helix/`, and the cache is `~/.cache/helix/`.

---

## 1. What a plugin is

- A **WebAssembly component** (component model, target `wasm32-wasip2`) that
  Wasmtime runs in a sandbox. Any language that can build a component for the
  WIT world `helix-plugin` works. Rust has an SDK (`helix-plugin-api`, in
  `helix-plugin/sdk`), and JavaScript has been verified (`examples/js-upper`).
- **No network, environment variables or standard streams.** The plugin can
  only touch its own data directory and, if the user grants it, the workspace
  and the `allow-dirs` directories.
- **Limits per call:** `init` gets 5 s, commands and callbacks 2 s, event
  handlers 250 ms, and background tasks 30 s. Memory is capped at 256 MiB. A
  plugin that runs out of time or memory, or traps (including a Rust
  `panic!`/`unwrap` failure), is **disabled until `:plugin-reload`**. The
  editor keeps running. Time limits also cover blocked WASI calls such as
  sleeping or opening a FIFO.
- **Call model:** the editor calls the plugin synchronously with a
  *snapshot* of the state: open documents, their text, paths, languages and
  selections, the focused document and the mode. Things the plugin changes
  (edits, selections, messages, opened files, commands, decorations) are
  recorded and **applied after the call returns, in order**:
  - All edits one call makes to a document are **one undo step**.
  - Edits are checked. They must be in bounds and must not overlap.
    Otherwise the call fails and nothing is applied.
  - Reads later in the same call see the plugin's own edits.
  - Edits a plugin makes do **not** trigger change events, so a plugin that
    reacts to changes cannot loop.
- **Positions are char offsets** (Unicode scalar values), not bytes. Rust
  strings are byte-indexed, so convert with `helix_plugin_api::text`
  (`char_len`, `char_to_byte`, `slice`, `line_starts`). A cursor in Pure is a
  one-char selection. `examples/text-tools` treats
  `anchor.abs_diff(head) <= 1` as "just a cursor".
- **Long-running work is asynchronous.** Programs, pickers, prompts, popups,
  timers, file operations and background tasks return a request id at once
  and report back through a callback export. The SDK turns this into
  one-shot closures.

### API versions and the compatibility rule

`plugin.toml`'s `api` field picks the WIT world the component was built
against. Pure loads all four versions side by side (host constant
`API_VERSION = "0.4"`).

| `api` | World adds | Status |
| --- | --- | --- |
| `0.1` | `host` import; `plugin` export | Frozen |
| `0.2` | `process`, `ui` imports; `callbacks`, `events` exports | Frozen |
| `0.3` | `decorations`, `worker`, `diagnostics`, `timers`, `completion`, `files` (fire-and-forget `create-path`/`rename-path`/`delete-path`) imports; `background` export | Frozen for exports |
| `0.4` | `files.apply` with results (`on-files-done` export), `sidebar` import | **Current: use this** |

The rule: **a released version may gain imports (host functions), never
exports.** A component that is missing an export fails to load, so any new
callback or event handler needs a new API version. That is why
`set-conceals`, `set-render-modes` and `set-line-backgrounds` exist in both
0.3 and 0.4. For new plugins, use `api = "0.4"` with the SDK's default
features. The manifest's `api` **must match** the world the component was
built for. A mismatch, or a stale build, fails at load time with "a matching
implementation was not found in the linker".

(`docs/plugin-progress.md`'s status line still says "0.3 is the current one".
That line is stale. The code, the roadmap and the template all use 0.4.)

---

## 2. Project layout and setup

```
my-plugin/
├── Cargo.toml
├── plugin.toml
├── plugin.wasm      # built component, committed (installs never build)
└── src/lib.rs
```

Start from the template if `cargo-generate` is available:

```sh
cargo generate --git https://github.com/TsaH0/Pure helix-plugin/template
```

### Cargo.toml

The examples in the repo use a path dependency
(`helix-plugin/examples/*/Cargo.toml`):

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]          # required

[dependencies]
# Inside the Pure repo (examples):   { path = "../../sdk" }
# Elsewhere, a local Pure checkout:  { path = "/path/to/helix-editor-custom/helix-plugin/sdk" }
# Or git (Cargo finds the package by name in the repo):
helix-plugin-api = { git = "https://github.com/TsaH0/Pure" }

[profile.release]                # what every example uses: small components
opt-level = "s"
lto = true
strip = true
codegen-units = 1

[workspace]                      # keep it out of any parent workspace
```

The template writes
`{ git = "...", subdirectory = "helix-plugin/sdk" }`. Cargo has no
`subdirectory` key for dependencies. It ignores the key with a warning and
still finds `helix-plugin-api` by name in the repo. The SDK reads the WIT
through the relative path `../wit/v0.4`, so it must stay inside a Pure
checkout (a path or git dependency works; a copied `sdk/` directory alone
does not).

SDK features (pick at most one; default = API 0.4): `api-v0-3`, `api-v0-2`,
`api-v0-1`. Use them only to rebuild an old plugin, and keep its manifest
`api` matching. The SDK depends only on `wit-bindgen = "0.60"`.

### plugin.toml (parsed with `deny_unknown_fields`: only these keys)

```toml
name = "my-plugin"          # required; lowercase letters, digits, '-' and '_'
version = "0.1.0"           # required
api = "0.4"                 # required; "0.1" | "0.2" | "0.3" | "0.4"
description = "What it does"            # optional
permissions = ["read-buffers", "edit-buffers"]  # optional, default []
wasm = "plugin.wasm"        # optional, default "plugin.wasm"; must be a file next to plugin.toml

# Optional default key bindings, shaped like config.toml's [keys]; values
# are THIS plugin's command names (checked: unknown names are an error).
[keys.normal.space]
P = "hello"                 # runs :my-plugin/hello; only fills keys nothing else binds

# Optional: bindings that apply only in buffers this plugin owns (open_buffer),
# where they take precedence. They cannot be turned off.
[keys.buffer.normal]
ret = "select"
```

Permissions (exact strings, from `Permission` in `helix-plugin/src/lib.rs`):

| Permission | Grants | Also needs a user grant in `plugins.toml` |
| --- | --- | --- |
| `read-buffers` | `text`, `text_range`, `selections`, `document_path`, `document_language`, `document_modified` | – |
| `edit-buffers` | `apply_edits`, `set_selections` | – |
| `open-files` | `open_file`, `open_file_at`, `open_scratch`, `open_buffer` | – |
| `run-commands` | `run_command` (only from commands/callbacks) | `allow-process = true` for commands that start programs (`:sh`, `:pipe`, `:pipe-to`, `:insert-output`, `:append-output`, `:run-interactive`, `!`, `:compile`, `:grep`, ...); a program list is not enough |
| `process` | `spawn`, `spawn_stream`, `run_interactive`, `kill` | `allow-process = true` or `["prog", ...]` |
| `filesystem` | workspace access through WASI `std::fs`, `files::*` | `allow-files = "read"` or `"write"` (+ optional `allow-dirs`) |
| `registers` | `register`, `set_register` (clipboard `+`/`*` write-only) | – |

Decorations, diagnostics, completion answers, timers, the sidebar, the
statusline, popups, prompts, pickers, status messages, logging and background
tasks need **no** permission. To compute decorations from the text you still
need `read-buffers`. Ask only for what the plugin uses.

### The user's `~/.config/helix/plugins.toml`

One table per plugin, keyed by the manifest `name`:

```toml
[my-plugin]
enabled = false                # or `my-plugin = false`: don't load it
allow-process = ["rg", "git"]  # or true (any program); checked against argv[0] as given
allow-files = "read"           # "read" | "write" | false
allow-dirs = ["~", "/tmp"]     # absolute dirs (~ expands) beyond the workspace
keys = false                   # ignore the plugin's [keys] (not [keys.buffer])
width = 80                     # anything else is passed to init as JSON
```

`init(config)` receives the table **minus** `enabled`, `allow-process`,
`allow-files`, `allow-dirs` and `keys`, as a JSON object string (`"{}"` when
there is none). The examples parse it by hand to stay dependency-free, for
example `config.split("\"max-line\":")`. Adding `serde_json` also works.

### Build

```sh
rustup target add wasm32-wasip2            # once
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/my_plugin.wasm plugin.wasm   # crate name, '-' -> '_'
```

(The repo pins Rust 1.90.0 in `rust-toolchain.toml`. The examples are built
with `cargo build --locked --release --target wasm32-wasip2`. Inside the repo,
`helix-plugin/build-plugins.sh` rebuilds every example and test fixture with
`--remap-path-prefix` so local paths stay out of committed components. Never
overwrite the version-suffixed frozen fixtures in `helix-plugin/tests/fixtures/`.)

### Install, reload, inspect

- **By hand:** put the files at `~/.config/helix/plugins/<name>/plugin.toml`
  and `.../plugin.wasm`. Plugins load in the background at startup, after the
  first frame. For documents opened meanwhile, plugins receive
  `document-open`.
- **`:plugin-install <git URL | local dir>`** shows the name, version and
  permissions and asks first. It copies only `plugin.toml` and the component
  named by `wasm`, both from the **repo root**, and builds nothing.
  `:plugin-update [names]` asks again if the permissions grew.
  `:plugin-remove <name>`.
- **`~/.config/helix/plugins.lock`** records source, commit and SHA-256 of
  each installed component. A changed component is refused until it is
  updated or reinstalled. Plugins copied in by hand are not in the lock.
- **`:plugin-reload`** reloads everything, re-enables disabled plugins and
  clears all plugin decorations and diagnostics. Plugins with `allow-files`
  are reloaded automatically when `:cd` changes the workspace.
- **`:plugin-list`** shows plugins, permissions, grants, commands, keys and
  the reason a plugin was disabled. **`:plugin-log <name>`** shows the last
  1000 `editor::log` lines. `pure --health` prints the plugin directory.
- Run commands with `:<plugin>/<command> [args]` or
  `:plugin-run <plugin>/<command> [args]`. They complete at the prompt, show
  in the command palette (`space ?`) and can be bound in `config.toml`
  (`u = ":my-plugin/hello"`).
  `editor.directory-handler = "oil/open"` makes `pure <dir>`/`:open <dir>`
  run a plugin command with the directory.

---

## 3. API reference (Rust SDK `helix_plugin_api`)

Re-exported at the crate root: `register_plugin!`, `Plugin`, `CommandInfo`,
`Registration`, `DocumentId` (u32), `Edit {from,to,text}`,
`Range {anchor,head}`, `EventKind`, `Mode {Normal,Insert,Select}`,
`OpenAction {Replace,Hsplit,Vsplit}`, `RequestId` (u32), `Output
{exit_code,stdout,stderr}`, `Item`, `Picker`, `Preview`, `Choice
{items,indices,action}`, `SpawnOptions`, `InteractiveOptions`,
`PopupOptions`. Modules: `editor`, `decorations`, `diagnostics`,
`completion`, `files`, `sidebar`, `text`.

### Skeleton (`examples/text-tools`, template)

```rust
use helix_plugin_api::{editor, register_plugin, CommandInfo, Edit, EventKind, Plugin, Registration};

struct MyPlugin;

impl Plugin for MyPlugin {
    fn init(_config: String) -> Result<Registration, String> {
        Ok(Registration {
            commands: vec![CommandInfo { name: "hello".into(), doc: "Insert a greeting".into() }],
            events: vec![EventKind::DocumentOpen],   // only subscribed handlers are called
        })
    }

    fn run_command(name: &str, args: Vec<String>) -> Result<(), String> {
        match name {
            "hello" => {
                let doc = editor::current_document().ok_or("no document")?;
                let cursor = editor::selections(doc)?[0].head;   // [0] is the primary
                let text = format!("hello {}", args.join(" "));
                editor::apply_edits(doc, &[Edit { from: cursor, to: cursor, text }])
            }
            _ => Err(format!("unknown command {name}")),   // Err is shown to the user
        }
    }
}

register_plugin!(MyPlugin);
```

Keep state in `thread_local!` `Cell`/`RefCell` statics, as the examples do.
The instance lives until reload.

### Call contexts: what is allowed where

| Context | UI (`pick`, `prompt`, `popup`, `run_interactive`) and `run_command` | Programs, timers, `run_background`, `files::apply`, sidebar, `push_items` |
| --- | --- | --- |
| `init` | no | no ("can't be used from init") |
| commands, callbacks (closures), timer callbacks, `on_buffer_write`, sidebar enter | yes | yes |
| event handlers (`on_*`) | **no** | yes |

Decorations, diagnostics, `completion::complete` and status/log work
everywhere except where noted. `:plugin-run` recursion is limited to a depth
of 4. When a command fails (returns `Err`), the closures it registered are
discarded and picker changes are rolled back.

### Commands

Commands are declared in `init` (`CommandInfo { name, doc }`) and dispatched
in `run_command(name, args)`. Return `Err(msg)` to show an error.

### Events (subscribe in `Registration.events`, implement the `Plugin` method)

| `EventKind` | Handler | Notes |
| --- | --- | --- |
| `DocumentOpen` | `on_document_open(doc)` | |
| `DocumentChange` | `on_document_change(doc)` | debounced (50 ms): a burst of typing becomes one call |
| `ModeSwitch` | `on_mode_switch(old, new)` | |
| `DocumentSave` | `on_document_save(doc)` | after writing |
| `DocumentClose` | `on_document_close(doc)` | doc already gone: only the id |
| `SelectionChange` | `on_selection_change(doc)` | debounced; **avoid it for rendering** (see §7) |
| `PostCommand` | `on_post_command(name)` | completed native command dispatch, e.g. `"yank"`, `"move_char_left"`; commands applied by plugins are not reported (no feedback loops) |
| `FocusGained` / `FocusLost` | `on_focus_gained()` / `on_focus_lost()` | |
| `EditorIdle` | `on_editor_idle()` | once per idle period; re-armed by input |
| `Completion` | `on_completion(id, doc, position)` | makes the plugin a completion source |
| (none) | `on_buffer_write(doc) -> Result` | `:write` on a buffer the plugin owns; runs as a command |
| (none) | `run_background(task, input) -> Result<String,String>` | runs in the background instance |

Unsubscribed events cost nothing: the editor checks an atomic bitmask.

### Buffers, text, editing, selections (`editor`, host interface)

- `current_document() -> Option<DocumentId>`, `documents() -> Vec<DocumentId>`,
  `current_mode() -> Mode`
- `text(doc)`, `text_range(doc, from, to)`, `selections(doc) -> Vec<Range>`,
  `document_path(doc) -> Option<String>`, `document_language(doc)`,
  `document_modified(doc) -> bool` (needs `read-buffers`)
- `apply_edits(doc, &[Edit])`: non-overlapping, with offsets into the text as
  it stands before this call. `set_selections(doc, &[Range])`. Both need
  `edit-buffers`.
- `set_status(&str)`, `set_error(&str)`, `log(&str)` (Helix log file plus
  `:plugin-log`)
- `run_command(":write")` or a static command name like `"goto_line_end"`
  (needs `run-commands`; commands and callbacks only)
- `workspace_root() -> Option<String>`, `cwd()`, `config_dir()`,
  `data_dir()` (`~/.local/share/helix/plugins/<name>/`, always read-write via
  `std::fs`, no permission needed)

### Opening files and buffers (`open-files`)

- `open_file(path)`, `open_file_at(path, OpenAction::Vsplit, Some(line), Some(col))`.
  Line and column are **1-based**, columns are in chars, and out-of-range
  values are clamped.
- `open_scratch(text, Some("diff"), OpenAction::Replace)`: a new scratch
  buffer that becomes the current document.
- `open_buffer(name, text, language, action)`: a buffer the plugin **owns**,
  shown as `name` (1–256 chars, one line; for example `oil:///work/`).
  Opening the same name again shows the existing buffer and ignores `text`.
  Afterwards `on_document_open(doc)` is called (no subscription needed).
  `buffer_name(doc) -> Option<String>` identifies the plugin's own buffers.
  `:write` calls `on_buffer_write(doc)`. Call `mark_saved(doc)` once the write
  is applied, or `:q` will warn. `[keys.buffer.*]` bindings apply in these
  buffers only. See `examples/oil`.

### Pickers (commands and callbacks only)

```rust
// examples/todos
let mut picker = Picker::new("Todos", items);          // 1 column "Name" by default
picker.columns = vec!["Line".into(), "Kind".into(), "Text".into()];
editor::pick(picker, move |choice| {
    let (Some(item), Some(action)) = (choice.items.first(), choice.action) else {
        return Ok(());                                  // action None = cancelled
    };
    editor::open_file_at(&path, action, Some(line + 1), None)
})?;
```

- `Item::text(s)` (display and data are both `s`), `Item::new(columns, data)`,
  `.with_file_preview(path, Some((first, last)))` (0-based inclusive lines),
  `.with_text_preview(text)`.
- Up to 8 columns and 1,000,000 items in total (initial plus pushes;
  `set_items` resets the count). One plugin picker is open at a time.
- `push_items(id, items)` and `set_items(id, items)` update an open static
  picker.
- Live picker: `pick_live(picker, |id, generation, query| -> Result<Option<Vec<Item>>, String>, on_choice)`.
  Return `Ok(Some(items))` to answer immediately. Return `Ok(None)` to answer
  later with `set_items_for_query(id, generation, items)` or
  `push_items_for_query(id, generation, items)`, which drop stale
  generations and closed pickers. Always pass the generation you were given.
  See `examples/finder` (`grep`).

### Popups and prompts (commands and callbacks only)

- `popup(markdown) -> RequestId` shows markdown at the cursor until the next
  key. `popup_with_options(md, PopupOptions { centered: true }, |id| ...)`
  calls back once when the popup closes. `close_popup(id)` closes it.
- `prompt(message, initial, completions, |answer: Option<String>| ...)` asks
  for one line of text (`None` on Esc). For yes/no, offer the completions
  `["yes","no"]`.

### Running programs (`process` permission plus `allow-process`)

- `spawn(&argv, SpawnOptions { cwd, stdin, timeout_ms }, |out: Result<Output,String>| ...)`.
  There is no shell, and the default working directory is the editor's cwd.
  The default timeout is 30 s, the maximum 10 min. stdout and stderr are each
  capped at 16 MiB (lossy UTF-8). `exit_code` is `None` if the program was
  killed or timed out.
- `spawn_stream(&argv, opts, |chunk| ..., |final| ...)` delivers complete
  stdout lines in batches. The final result holds **only** the unfinished
  tail and the full stderr, so do not append the batches again.
- `kill(id)`: the final callback is still delivered. At most 16 programs run
  at once per plugin. Reloading kills them.
- `run_interactive(&argv, InteractiveOptions { cwd, chooser_file, wait_for_key }, cb)`
  hands the whole terminal to a program. With `chooser_file`, an argument
  containing `{chooser}` becomes a private temp file whose contents arrive
  as `stdout`. Commands and callbacks only.

```rust
// examples/finder (files): stream rg into an already-open picker
let process = editor::spawn_stream(&["rg", "--files"], options,
    move |chunk| { editor::push_items(id, file_items(&root, chunk)) },
    move |output| { let output = output?; /* push output.stdout tail */ Ok(()) })?;
```

### Files (`filesystem` permission plus `allow-files`)

- Read or write the workspace with plain `std::fs` (WASI preopens use real
  host paths, so paths from `document_path` and `workspace_root` work). A
  workspace counts only if it is a real project (with `.git`, `.helix`, ...),
  never `$HOME`, `/` or a directory that contains Pure's config or data
  directories.
- `files::roots()` returns the workspace plus the granted `allow-dirs`.
- API 0.4 `files::apply(&ops, |results| ...)` changes files **through the
  editor**, so buffers follow renames and language servers are notified.
  Use it instead of `std::fs` for renames and moves of open files. Needs
  `allow-files = "write"`. Paths are absolute and inside `roots()`, with no
  `..` and no symlink escapes. Nothing overwrites an existing path. At most
  1000 operations run in order and stop at the first failure; later
  operations fail with "skipped".

```rust
use helix_plugin_api::files::{self, CreateOp, FileOp, MoveOp};
files::apply(&[
    FileOp::Create(CreateOp { path: "/work/src/new.rs".into(), directory: false }),
    FileOp::Rename(MoveOp { from: "/work/a.rs".into(), to: "/work/src/a.rs".into() }),
    FileOp::Trash("/work/old".into()),     // also Copy(MoveOp), Delete(path)
], |results| { for r in results { r?; } editor::set_status("done"); Ok(()) })?;
```

### Decorations (`decorations`; no permission; works in event handlers)

Each kind is stored per **layer** (a name you choose, 1–64 chars, no `/`).
Setting a layer **replaces** it, and an empty slice clears it. A layer holds
at most **100,000** decorations. Offsets are chars into the text the call
sees, and decorations follow the user's edits until you set the layer again.
Reloading clears them.

| Function (SDK builder) | What |
| --- | --- |
| `set_highlights(doc, layer, &[highlight(from, to, "warning")])` | theme-scope highlight; on overlap within a layer, the first wins |
| `set_annotations(doc, layer, &[annotation(pos, "  ◂ todo", "comment")])` | virtual text before char `pos`; at a line's newline offset it goes at the end of the line; no line breaks in the text |
| `set_overlays(doc, layer, &[overlay(pos, "◉", scope)])` | chars drawn **over** the text, one per char (text unchanged) |
| `set_signs(doc, layer, &[sign(line0, "●", "warning")])` | one char in the `plugins` gutter (0-based line) |
| `set_line_backgrounds(doc, layer, &[line_background(line0, "markup.raw.block,ui.virtual.ruler")])` | full-width line background; comma-separated fallback scopes |
| `set_conceals(doc, layer, &[conceal(from, to, "", scope)])` | hide text (within one line); replacement is empty or one grapheme |
| `set_raw_on_cursor_line(doc, layer, true)` | hide the layer on the primary cursor's line; the editor tracks the cursor |
| `set_render_modes(doc, layer, &[Mode::Normal])` | draw the layer only in these modes (empty = all) |
| `set_statusline(segment, text)` | text in the `plugins` statusline element; segment 1–64 chars, no `/`; text ≤ 200 chars, control chars become spaces; empty removes the segment |

Users must opt in to see signs and statusline text:
`[editor] gutters = ["diagnostics", "plugins", "line-numbers", "spacer", "diff"]`
and `[editor.statusline] right = ["plugins", ...]`.

```rust
// examples/markdown: render-markdown.nvim behaviour without cursor events
for layer in ["text", "marks", "rules", "inline", "conceal"] {
    decorations::set_raw_on_cursor_line(doc, layer, true)?;
}
for layer in ["text", "marks", "rules", "inline", "conceal", "blocks"] {
    decorations::set_render_modes(doc, layer, &[Mode::Normal])?;
}
decorations::set_conceals(doc, "conceal", &rendered.conceals)?;
```

### Diagnostics (`diagnostics`; shown like an LSP's: gutter, inline, `]d`, `space d`)

```rust
// examples/lint
let mut d = diagnostic(from, to, Severity::Warning, "indentation mixes tabs and spaces");
d.code = Some("mixed-indent".into());
diagnostics::set_diagnostics(doc, &found)?;   // replaces this plugin's set; &[] clears
```

There is one set per plugin per document. `source` defaults to the plugin
name. Messages are cut at 4096 chars, and a document holds at most 100,000
diagnostics. Ranges are checked against the document length.

### Completion (`completion`)

Subscribe to `EventKind::Completion` and implement
`on_completion(id, doc, position)`. Answer **exactly once** with
`completion::complete(id, &items)` (at most 1000 items), either immediately
or from a later callback. The menu waits up to 1 s, and a new request
replaces an unanswered one. Always answer, even with an empty list.
`completion::item(label)` plus the optional `insert`, `kind`,
`documentation` (markdown) and `replace_from` (char offset; default: the
start of the word before the cursor). The editor filters items by what the
user typed. See `examples/emoji`.

### Snippets, revisions, cancelling (added to API 0.4)

Imports only, so 0.4 plugins built before still load; a plugin using them
needs a Pure that has them (`editor::capabilities()` lists
`"cancel-request"`, `"revisions"`, `"snippet-completions"`; on an older
Pure such a plugin fails to load with "needs a newer Pure").

- `completion::complete_snippets(id, &items)`: like `complete`, but
  `insert` (or `label`) is an LSP-style snippet (`$1`, `${2:x}`, `${3|a,b|}`,
  `$0`), expanded by the editor's snippet engine with tab stops. Snippet
  variables (`${TM_FILENAME}`) are not resolved. See `examples/snippets`.
- `editor::revision(doc) -> u64` (needs `read-buffers`) and
  `editor::apply_edits_at(doc, revision, &edits)` (needs `edit-buffers`):
  apply only if the text is unchanged since `revision`. Always use this
  for edits computed asynchronously (background tasks, programs, timers).
  See `api-demo`'s `sort`.
- `editor::cancel(id)`: stop any request and drop its callback (timer,
  program, background task, file batch, completion, popup; pickers and
  prompts stay open but their answer is ignored).

### Timers (`editor::set_timer`)

`set_timer(ms, || -> Result<(), String>)` runs the closure once. Set
another timer from inside it to repeat. `cancel_timer(id)` stops a timer.
At most 16 timers per plugin, and none from `init`. Timer callbacks are
callbacks, so UI and commands are allowed. Nothing runs or wakes while no
timer is set. See `examples/pomodoro`.

### Registers (`registers` permission)

`register('"') -> Vec<String>` returns one value per selection, or an empty
list when the register is unset. `set_register('"', &values)` needs at least
one value. Clipboard registers `+`/`*` can be written but **not read**. See
`examples/yank-history` (it listens to `PostCommand` for `yank*`).

### Sidebar (API 0.4, `sidebar`)

```rust
use helix_plugin_api::sidebar::{self, item, on_enter, SidebarView};
let file = on_enter(item("  main.rs", "", 1), "open", vec!["/work/src/main.rs".into()]);
sidebar::show(&SidebarView { title: " work/".into(), items: vec![file], width: 32, selected: None })?;
sidebar::focus(true);
```

Items have a text, a theme scope (empty means plain) and a depth (2 columns
each), plus an optional command of **this plugin** to run with `args` on
Enter, `l` or a click. Width is 12–80 columns (at most half the window), and
a sidebar holds at most 100,000 items. Only one sidebar is shown at a time;
the plugin that showed it last owns it. Showing it again updates it and
keeps scroll and selection. `close()`, `focus(bool)` and
`selected() -> Option<u32>` are also available. Keys while focused:
`j`/`k`/arrows, `ctrl-d`/`ctrl-u`, `g`/`G`; `esc`/`tab` give the keys back,
and `q` closes the sidebar. See `examples/tree`.

### Background calls (`editor::run_background`)

```rust
editor::run_background("outline", editor::text(doc)?, move |result| {
    editor::set_status(&result?);   // back in the main instance
    Ok(())
})?;
// in impl Plugin:
fn run_background(task: &str, input: String) -> Result<String, String> {
    match task { "outline" => Ok(compute_outline(&input)), _ => Err(format!("unknown task {task}")) }
}
```

The task runs in a **second instance** on another thread with up to 30 s.
That instance has its own memory (statics are not shared) and **no editor
access**: documents read as missing and actions are dropped. Pass everything
it needs in `input` and return a string. At most 4 tasks run at once. A task
that crashes or times out returns an error and the plugin stays enabled.
Allowed from commands, callbacks and event handlers.

### Key bindings

Key bindings are `[keys.<mode>...]` in `plugin.toml`, with values that are
the plugin's own command names. They only fill keys that neither the user's
config nor Pure's defaults bind, and the user can turn them off with
`keys = false`. `[keys.buffer.<mode>]` applies only in the plugin's own
buffers. Key names follow `config.toml` (`minus`, `ret`, `C-s`, `"_"`, ...).

### Other languages

The plugin must export every interface of the world (`plugin`, `callbacks`,
`events`, `background`); empty handlers are fine.
- JavaScript (verified): `jco componentize plugin.js --wit ../../wit/v0.3
  --world-name helix-plugin --disable http --disable fetch-event -o
  plugin.wasm` (`examples/js-upper`, `api = "0.3"`). The result is about
  12 MB, and the first load compiles slowly before the cache is used.
- Python (`componentize-py`) and Go (TinyGo + `wit-bindgen-go`) are
  documented in the README but **untested**.

---

## 4. Permissions and security rules

- Request the **minimum** permissions. Anything that escapes the sandbox
  (programs, files) needs the manifest permission **and** the user's grant.
  Without the grant, calls fail with a message that says what to add.
- `allow-process` is a **trust decision, not a sandbox**. An allowed program
  can start others (`rg --pre`, `git -c alias.x=!sh`) with the user's full
  rights, including the network. Do not tell users to grant
  `allow-process = true` when a short list works. Never pass user-controlled
  strings as program options without validation.
- A workspace `write` grant can change git hooks, and `run-commands` can run
  `:write` and similar commands. Request them only when needed.
- Prefer `files::apply` (checked against the roots, never overwrites, `Trash`
  never falls back to deleting) over raw `std::fs` writes for user files.
- There is no network. To fetch something, run a program such as `curl` with
  `allow-process`, and document the need.
- Never `panic!` or `unwrap()` on host results or user input. A trap
  disables the plugin. Return `Err(String)` from commands, and in event
  handlers log the error (`editor::log`).

---

## 5. What is possible and what is not

Possible (with an example in `helix-plugin/examples/`):

| Kind (Neovim analogue) | How | Example |
| --- | --- | --- |
| Text transforms, commands | `apply_edits`, `set_selections` | `text-tools`, `js-upper` |
| Finders, pickers (telescope) | `pick`, `pick_live`, `spawn_stream` | `finder` |
| Linters (nvim-lint) | `diagnostics::set_diagnostics` | `lint` |
| Completion sources (nvim-cmp) | `on_completion` + `complete` | `emoji` |
| File managers (oil.nvim) | `open_buffer`, `on_buffer_write`, `[keys.buffer]`, `files::apply`, `allow-dirs` | `oil` |
| File trees, outlines (nvim-tree) | `sidebar` (API 0.4) | `tree` |
| todo-comments, render-markdown | highlights, annotations, overlays, signs, conceals, line backgrounds, render modes | `todos`, `markdown` |
| Timers, clocks, auto-save | `set_timer` | `pomodoro` |
| Yank history (yanky.nvim) | registers + `PostCommand` | `yank-history` |
| Output/REPL buffers | `open_scratch` | – |
| Git and other tools | `spawn`, `run_interactive` | `finder` (branches) |
| Statusline segments, popups, prompts, key bindings, heavy background work | see §3 | `api-demo` |

Not possible (from the roadmap's M12 table):

| Kind | Status |
| --- | --- |
| Panes other than the one sidebar (bottom panels, floating windows with their own keys) | not possible |
| Embedded terminals (toggleterm) | not possible: Helix has no terminal emulator; `run_interactive` hands over the whole terminal instead |
| Folding | not possible: Helix has no folds |
| Custom UI components (dashboards with layout) | only popups (markdown), pickers, prompts and the sidebar |
| Debug adapters, language servers | use Helix's DAP/LSP via `languages.toml` |
| Network access | not in the sandbox; run a program with `allow-process` |
| Format on save | use `formatter` in `languages.toml`; a plugin can offer a format command |
| Changed-range info in `on_document_change`, partial layer updates | not in the API: re-read the text, diff it yourself, replace whole layers |

---

## 6. Testing

1. **Unit-test the pure logic natively.** A plugin crate builds for the host
   target too, so `cargo test` in the plugin directory runs `#[cfg(test)]`
   tests (verified with `examples/lint`). Host functions are unavailable
   natively, so keep parsing and rendering in plain functions and test those.
   See `lint`, `emoji`, `pomodoro`, `yank-history`, `oil` and `markdown`
   (which has a property test comparing incremental and full renders).
2. **Build the component** and fix every warning:
   `cargo build --release --target wasm32-wasip2`.
3. **Inside the Pure repo** (`export HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1`
   first, since grammars cannot be fetched in sandboxes):
   - `helix-plugin/build-plugins.sh` after changing an example, the SDK or
     the WIT.
   - `cargo test -p helix-plugin` runs host tests that load real components:
     one per interface (`api_03_*`) and per example (`*_example_*`, via
     `load_example(&mut host, root, "<name>", "<json config>")` plus
     `host.dispatch(Event::DocumentOpen(1), &snapshot(..))`, then asserting
     on the returned `Action`s).
   - `cargo test -p helix-term --features integration --test integration -- plugins`
     runs in-editor tests (`helix-term/tests/test/plugins.rs`, for example
     `app_with_example("<name>", "<plugins.toml>")`, `keys(&mut app,
     ":plugin-run x/y<ret>")`, `wait_for_status`, `deliver_events`). Call
     `app.editor.reset_idle_timer()` before waiting without input.
   - `cargo clippy --workspace --all-targets`, `cargo fmt --all`.
4. **Manually, against the real editor**, with a throwaway config:

   ```sh
   export XDG_CONFIG_HOME=$(mktemp -d)        # also consider XDG_DATA_HOME / XDG_CACHE_HOME
   mkdir -p $XDG_CONFIG_HOME/helix/plugins/my-plugin
   cp plugin.toml plugin.wasm $XDG_CONFIG_HOME/helix/plugins/my-plugin/
   printf '[my-plugin]\n' > $XDG_CONFIG_HOME/helix/plugins.toml   # grants/settings
   pure some-file.txt     # then :plugin-list, :my-plugin/<cmd>, :plugin-log my-plugin
   ```

   - Run it inside `tmux` to script it (`tmux new -d -s t 'pure file'`,
     `tmux send-keys -t t ...`, `tmux capture-pane -p -t t`). Do not use
     `pkill -f`/`pgrep -f`, which match your own shell.
   - The integration test build overwrites `target/debug/pure` with a
     test-terminal build that draws nothing. Run
     `touch helix-term/src/main.rs && cargo build` before trying it by hand.
   - For `allow-files`, run from a real project directory (with `.git`).
   - If themes or grammars are missing, set `HELIX_RUNTIME=<pure repo>/runtime`.
   - To check that a crash is contained, break something on purpose:
     `:plugin-list` shows the plugin as disabled with the reason, and the
     editor keeps working.

---

## 7. Performance

The owner's rule: **plugins must never make Pure slower than vanilla Helix**,
and nothing plugin-related runs at startup, per keystroke or while idle unless
a plugin asked for it.

Recorded numbers (`docs/plugin-progress.md`, release builds, 4 cores):

| Measurement | Result |
| --- | --- |
| Pure without plugins vs vanilla Helix (7,500-line Rust file) | startup 447 vs 450 ms; editing CPU 22.16 vs 22.21 s; 0 idle wake-ups for both; +1.2 MiB memory |
| Startup to first frame with 5 or 10 plugins | unchanged (443–451 ms): plugins load after the first frame |
| CPU to start with 10 plugins | about +50 ms (instantiation, after the first frame); the first run after install compiles once (up to about 2.5 s background CPU, then cached) |
| Idle with plugins | 0 wake-ups over 5 s |
| Memory per loaded plugin | about 2–3 MiB (10 plugins: +22 MiB) |
| Load budget | under **5 ms per cached plugin**; warm load 1 / 5 / 20 plugins ≈ 1.6–2.8 / 3.9–6.9 / 14.9–28.9 ms; cold (compiling) ≈ 57–95 / 231–273 / 932–1141 ms |
| Dispatching one event to one plugin (10,000-line doc) | 0.05–0.16 ms |
| Editing a 6,700-line Markdown file with markdown + lint + todos + emoji | about 2.8 ms per edit in total, under one frame; markdown alone about 1.5 ms per edit |
| Mistake to avoid | markdown first re-rendered on every cursor move (15.7 ms each, 1,846 ms in the session); fixed by a faster renderer and `set_raw_on_cursor_line` |

Benchmarks in the repo (there is no criterion and no cargo-fuzz; fuzzing is
quickcheck properties plus `fuzzing_through_the_abi` in
`helix-plugin/src/test.rs`):

```sh
cargo test -p helix-plugin --release -- --ignored --nocapture benchmarks      # load + dispatch
cargo test -p helix-plugin --release -- --ignored --nocapture markdown_cost   # one re-render
python3 docs/bench/vs-helix.py '{"helix": ["<hx>", "<runtime>"], "pure": ["<pure>", "<runtime>"]}' start|work|idle <rounds>
#   needs docs/bench/work/big.rs and a docs/bench-cfg config dir (see the script's top)
```

To measure your own plugin in the repo, copy `markdown_cost`: load it with
`load_example`, dispatch `Event::DocumentChange` on a large snapshot and time
it.

Rules:
- **Subscribe only to events you need.** Unsubscribed events cost nothing.
  Prefer `DocumentChange` (debounced) over `SelectionChange` or
  `PostCommand`. Never re-render on cursor moves: use
  `set_raw_on_cursor_line` and `set_render_modes`, which the editor applies
  without calling the plugin.
- **Keep event handlers far below their 250 ms limit**. Aim for well under
  one frame (about 16 ms) on large files. Do incremental work: cache per
  document (`thread_local!` map keyed by `DocumentId`) and re-process from
  the first changed line, as `examples/markdown` does. Read only what you
  need (`text_range` around the cursor, as `emoji` does, instead of `text`).
- **Batch decorations**: build all items, then make one `set_*` call per
  layer. Do not set a layer per item. Each layer holds at most 100,000
  decorations, so split very large sets across layers or render only what
  matters. Skip `set_statusline` when the document is not the current one
  (`todos` does this). Statusline text is drawn every frame, so keep it short
  (≤ 200 chars).
- **Move slow work off the call**: `run_background` for CPU-heavy parsing or
  indexing (up to 30 s, 4 at once), and `spawn`/`spawn_stream` for external
  tools. Kill obsolete programs (`kill`) when a picker closes or a query
  changes (see `finder`).
- **No polling.** Use `set_timer` only when you need a clock, and stop it
  when idle. Prefer `EditorIdle` or events otherwise.
- Do no filesystem walks or full-document scans per call without caching.
- Keep components small: use the release profile above (`opt-level = "s"`,
  `lto`, `strip`, `codegen-units = 1`).

---

## 8. Checklist before you declare the plugin done

- [ ] `plugin.toml` has only the allowed keys (`name`, `version`, `api`,
      `description`, `permissions`, `wasm`, `keys`). `name` is lowercase
      `[a-z0-9_-]` and matches the directory name. `api = "0.4"` (or matches
      the SDK feature used).
- [ ] `permissions` lists exactly what the code calls, and nothing more. The
      README tells users which `plugins.toml` grants are needed
      (`allow-process` list, `allow-files`, `allow-dirs`) and which settings
      exist.
- [ ] `Cargo.toml` has `crate-type = ["cdylib"]`, an empty `[workspace]`,
      the SDK dependency and the release profile.
- [ ] `cargo build --release --target wasm32-wasip2` succeeds with no
      warnings, and the output was copied to `plugin.wasm` (the `wasm` file
      named in the manifest) **next to `plugin.toml` at the repo root**, then
      committed.
- [ ] `cargo test` passes for the native unit tests of the pure logic (offsets
      in chars, empty documents, CRLF, Unicode, last line without a newline).
- [ ] No `unwrap`/`expect`/`panic!` on host results or input. Commands return
      `Err(msg)`, and event handlers log errors.
- [ ] UI, `run_command` and `run_interactive` are used only from commands
      and callbacks. Nothing that is disallowed in `init` is called from
      `init`.
- [ ] Every completion request is answered. Every live-picker result carries
      its generation. Programs are killed when their picker or query is gone.
      `mark_saved` is called after a successful `on_buffer_write`.
- [ ] Event handlers stay incremental and fast on a large file (thousands of
      lines). Decorations are batched per layer, within the 100,000 limit.
      No per-cursor-move work, and no timers while idle.
- [ ] Tested by hand in `pure` with a scratch `XDG_CONFIG_HOME`:
      `:plugin-list` shows it loaded with the right permissions and grants;
      every command works; `:plugin-log <name>` has no unexpected errors;
      `:plugin-reload` works; the plugin's `[keys]` do not override existing
      bindings.
- [ ] If the plugin lives in the Pure repo: `build-plugins.sh` was run,
      a host test (`*_example_*`) and, where useful, an in-editor test were
      added, `cargo test -p helix-plugin` and the `plugins` integration tests
      pass, frozen fixtures are unchanged, and the example is listed in
      `helix-plugin/README.md` and `book/src/plugins.md`.
