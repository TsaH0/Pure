# Plugin roadmap and task list

The plan for growing Pure's WebAssembly plugin system from "text tools" into
something closer to Neovim's (popups, pickers, file managers, git UIs), split
into tasks that can be handed to an agent or contributor one at a time.

- What exists today: [`helix-plugin/README.md`](../helix-plugin/README.md)
  (user and author guide) and [`plugin-system.md`](plugin-system.md) (the
  original design; phases 1–3 are done).
- The detailed design for the next two milestones (M1, M2):
  [`plugin-processes-and-pickers.md`](plugin-processes-and-pickers.md). Read
  it before starting M0–M2.

Each task has an id (`M1.2`), what it depends on, the files it touches, what to
do, and **done when** criteria. Tick the box in this file in the same PR that
finishes the task. Progress notes are in [`plugin-progress.md`](plugin-progress.md).

**Scope (the owner's rules):**

- **Pure ships the plugin API, not built-in plugins.** No file manager, yazi
  integration, git UI or other end-user feature goes into the editor itself:
  users build those as plugins.
- **Plugins written while working (to try an API) are kept** as opt-in
  examples in `helix-plugin/examples/<name>/` (source, `plugin.toml` and the
  built `plugin.wasm`), so users can copy them into their plugins directory.
  Never delete them; `build-plugins.sh` rebuilds every example.

---

## 0. Brief for whoever picks up a task

### Where things are

| Path | What |
| --- | --- |
| `helix-plugin/wit/helix-plugin.wit` | The API (WIT), package `helix:plugin@0.1.0` |
| `helix-plugin/src/lib.rs` | Host: `PluginHost`, `Manifest`, `Permission`, `Snapshot`, `Action`, `Event`, `Limits`, host functions (`impl host::Host for State`) |
| `helix-plugin/src/test.rs` | Host unit tests (32), using `tests/fixtures/testplugin.wasm` and `examples/text-tools/plugin.wasm` |
| `helix-plugin/tests/plugins/testplugin` | Source of the test plugin (its own cargo workspace) |
| `helix-plugin/sdk` | Rust guest SDK `helix-plugin-api` (`Plugin` trait, `register_plugin!`, `editor`, `text`) |
| `helix-plugin/examples/text-tools` | Example plugin; `plugin.wasm` is committed |
| `helix-plugin/build-plugins.sh` | Rebuilds the test plugin and example wasm files |
| `helix-term/src/plugins.rs` | Editor integration: loading, `snapshot`, `apply` (turns `Action`s into edits), hooks, debouncing, `:plugin-*` helpers |
| `helix-term/src/commands/typed.rs` | `:plugin-run`, `:plugin-list`, `:plugin-reload` |
| `helix-term/src/interactive.rs` | Handing the terminal to a program (`:run-interactive`, `space z`, plugins' `run-interactive`) |
| `helix-term/src/plugins/process.rs`, `plugins/picker.rs` | Programs and pickers started by plugins |
| `helix-term/src/ui/picker.rs` | The picker (`Picker::new`, `with_preview`, `injector`, `with_dynamic_query`) |
| `helix-term/tests/test/plugins.rs` | In-editor plugin tests |

### How plugin calls work (read this first)

The editor builds a `Snapshot` of the state a plugin may read and calls the
plugin **synchronously** with time and memory limits (`Limits`). The call runs
on a scoped thread outside Tokio (`outside_async_runtime`), because WASI's
sync bindings call `block_on`. Writes made by the plugin are recorded as
`Action`s and applied by `helix-term/src/plugins.rs::apply` after the call
returns. An `APPLYING` guard stops an edit made by a plugin from re-triggering
plugin events. Every new ability follows this pattern: **a host function
records an `Action`; `apply` performs it.** Anything long-running returns a
request id at once and reports back through a callback export (see the
design doc).

### Rules

- **Keep plugin code isolated.** New logic goes in `helix-plugin` or
  `helix-term/src/plugins.rs` (or new files next to it). Touch upstream files
  (`editor.rs`, `picker.rs`, `commands.rs`, ...) only with small, clearly
  named hooks, so merging upstream Helix stays cheap.
- **Never break existing plugins.** A component built for API `0.1` must keep
  loading. Changes to an existing WIT version are not allowed; add a new
  version.
- **Plugins are sandboxed by default.** Anything that escapes the sandbox
  (processes, files, network) needs a permission in `plugin.toml` **and** a
  grant by the user in `plugins.toml`.
- **Don't change the user's key bindings** or existing commands without asking
  (the owner's config uses `:sh`, `space x` and `space z`).
- Rust is pinned to **1.90** (`rust-toolchain.toml`), which is why Wasmtime is
  at **41.0.4**. Don't bump either without checking that everything builds.

### Commands

```sh
export HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1     # grammars can't be fetched in CI sandboxes

cargo test -p helix-plugin                    # host unit tests
cargo test -p helix-term --lib
cargo test -p helix-term --features integration --test integration -- plugins
cargo clippy --workspace --all-targets
cargo fmt --all
cargo xtask docgen                            # after adding/changing commands

rustup target add wasm32-wasip2               # once
helix-plugin/build-plugins.sh                 # after changing the test plugin, the SDK or an example
```

There are **19 integration test failures** that are not ours: tree-sitter tests
that need grammars, and write-permission tests that fail when run as root.
Compare against this list rather than expecting all green:

- `commands::surround_replace_ts`, `commands::test_insert_with_indent`
- `commands::write::{test_symlink_write_fail, test_write_fail_new_path, test_write_fail_mod_flag, test_write_quit_fail}`
- `commands::movement::{match_bracket, select_all_siblings, select_all_children, test_move_parent_node_end, test_move_parent_node_start, test_select_next_sibling, test_select_prev_sibling, test_smart_tab_move_parent_node_end}`
- `movement::{match_around_closest_ts, select_mode_tree_sitter_next_function_is_union_of_objects, select_mode_tree_sitter_prev_function_goes_backwards_to_object, select_mode_tree_sitter_prev_function_unselects_object, tree_sitter_motions_work_across_injections}`

The full run aborts (SIGABRT, a stack overflow) without a bigger stack, and
the 8 `commands::movement` tests can still abort it. Run the full suite as:

```sh
cargo test -p helix-term --features integration --test integration --no-run
B=$(ls -t target/debug/deps/integration-* | grep -v '\.d$' | head -1)
RUST_MIN_STACK=16777216 $B --test-threads 4 --skip test::commands::movement
```

Expect exactly the other 11 known failures.

Commit the rebuilt `.wasm` fixtures with the code that needs them. Manual
checks run in `tmux` (`tmux new -d -s t 'target/debug/pure'`,
`tmux send-keys`, `tmux capture-pane -p`).

---

## M0. API 0.2 groundwork

Nothing user-visible: the plumbing every later milestone needs.

- [x] **M0.1 Versioned WIT layout.** Depends on: nothing.
  - Move `wit/helix-plugin.wit` to `wit/v0.1/helix-plugin.wit` (unchanged);
    add `wit/v0.2/` with the 0.2 package from the design doc (`types` with
    `request-id` and `open-action`, plus `host`, `process`, `ui`,
    `callbacks`). The SDK and test plugin point at the new paths.
  - Done when: the 0.1 fixtures still build byte-for-byte compatible bindings
    and all `helix-plugin` tests pass.
- [x] **M0.2 Two worlds in the host.** Depends on: M0.1.
  - `bindgen!` once per version in `mod v0_1` / `mod v0_2`. Move the
    host-function bodies into plain methods on `State`, so both worlds'
    `Host` impls are thin wrappers (no duplicated logic).
  - `Plugin` holds an enum of the two instance types; `run_command`,
    `dispatch` and `init` match on it. `Manifest.api` picks the world:
    `"0.1"` or `"0.2"`; anything else is a load error naming the supported
    versions.
  - Done when: the 0.1 test plugin and `text-tools` load and pass every
    existing test, and a minimal 0.2 test plugin loads.
- [x] **M0.3 Request ids and callbacks.** Depends on: M0.2.
  - In `State`/`Plugin`: a `next_id` counter and `pending: HashMap<u32,
    PendingKind>`. Add `pub enum Callback { ProcessDone { id, result },
    PickerDone { id, result }, PromptDone { .. } }` and
    `PluginHost::callback(plugin, Callback, &Snapshot) -> Vec<Action>`, which
    calls the matching export with the command time limit and removes the
    id. Unknown ids are ignored (the plugin was reloaded).
  - Track a **context** per call (`Init`, `Command`, `Event`, `Callback`) so
    host functions can refuse UI from event handlers (as `run-command`
    already does).
  - Done when: a unit test fakes a callback for a pending id, gets the
    plugin's actions back, and an unknown id is a no-op.
- [x] **M0.4 SDK 0.2.** Depends on: M0.1.
  - `helix-plugin-api` 0.2: `thread_local!` registry `id → Box<dyn FnOnce>`;
    `register_plugin!` also exports `callbacks` and routes each callback to
    its closure. Public API sketch in the design doc (`editor::spawn`,
    `editor::run_interactive`, `editor::pick`). Keep 0.1 trait methods
    source-compatible.
  - Done when: `text-tools` builds against SDK 0.2 unchanged except for
    `api = "0.2"`, and the SDK has doc comments for every public item.
- [x] **M0.5 0.2 test plugin.** Depends on: M0.2, M0.4.
  - `tests/plugins/testplugin2` with commands that exercise each new host
    function (added as later tasks land). Add it to `build-plugins.sh`;
    commit `tests/fixtures/testplugin2.wasm`.

## M1. Processes and the terminal

Lets users write plugins that run programs: file managers, git tools,
linters, fuzzy finders.

- [x] **M1.1 `process` permission and grant.** Depends on: M0.2.
  - `Permission::Process` (`"process"` in `plugin.toml`). In
    `helix-term/src/plugins.rs`, read `allow-process = true | [programs]`
    from the plugin's `plugins.toml` table (and strip it before the config
    goes to `init`). The host stores the grant per plugin.
  - Without the grant, process functions return
    `"process not granted: add allow-process = true under [<name>] in plugins.toml"`.
    `:plugin-list` and `--health` show "process: granted / not granted /
    only rg, fd".
  - **Open decision for the owner:** whether to also ask once per program at
    first use (like workspace trust). Implement the config grant; leave a
    hook for the prompt.
- [x] **M1.2 `spawn` and `kill`.** Depends on: M0.3, M1.1.
  - Host function records `Action::Spawn { plugin, id, argv, options }`.
    `apply` runs it as a `tokio::process::Command` (no shell; `argv[0]`
    looked up in `PATH` and checked against the allow-list) on a task, with
    stdin written then closed, the timeout (default 30 s, max 10 min), and
    16 MiB caps on stdout and stderr (truncate and note it in stderr).
  - On exit: `job::dispatch` a callback that builds a snapshot, calls
    `PluginHost::callback(.., ProcessDone)` and `apply`s the result.
  - `kill(id)` kills the child; the callback still arrives with
    `exit-code: none`. At most 16 running processes per plugin. Reload and
    disable kill the plugin's processes.
  - Tests: `sh -c 'echo hi; echo err >&2; exit 3'` → exit 3 and both
    streams; stdin round trip (`cat`); timeout; kill; output cap; grant
    missing; program not in the allow-list; actions from the callback
    apply (e.g. it sets the status).
- [x] **M1.3 `run-interactive`.** Depends on: M0.3, M1.1.
  - `plugins::process::interactive(..) -> Interactive`, replacing a
    `{chooser}` argument with a fresh private temp file. (The built-in yazi
    command was removed: file managers are plugins.)
  - `Action::RunInteractive` → `interactive::request`; `on_exit` reads the
    chooser file (or nothing), deletes it and delivers `ProcessDone`.
    `wait-for-key` reuses the `space z` path (`shell_command_and_wait`
    style).
  - Only allowed in `Command` / `Callback` context.
  - Tests: chooser substitution and cleanup as unit tests; manual tmux check
    with a script that writes to the chooser file; refusal from an event
    handler.
- [x] **M1.4 Docs.** Depends on: M1.2, M1.3.
  - No example plugins (see Scope). `helix-plugin/README.md` has a "Running
    programs" section with the grant setup and short `spawn` /
    `run-interactive` snippets (a file chooser, a background command).

## M2. Pickers

Lets users write Telescope-style plugins: file finders, project switchers,
git pickers.

- [x] **M2.1 Picker close hook (upstream-file change).** Depends on: nothing.
  - `Picker` has no way to report "closed without choosing". Add an optional
    `on_close: Option<Box<dyn FnOnce(&mut Context)>>` (`with_on_close`) that
    runs on Esc/C-c, not on accept. Keep the patch small and self-contained
    in `ui/picker.rs`.
- [x] **M2.2 `show-picker`.** Depends on: M0.3, M2.1.
  - `Action::ShowPicker` → `Picker<PluginItem, PluginPickerData>` with the
    plugin's column headers, `with_initial_cursor`, and `with_preview` for
    `preview::file` (path plus line range) and `preview::text` (show text in
    a scratch preview; add a small `PathOrId`-style option if needed).
  - Accept: map the picker `Action` (Replace / HorizontalSplit /
    VerticalSplit) to `open-action` and deliver
    `PickerDone(accepted([index], action))`; close: `cancelled`.
  - Upstream's picker is single-select, so `accepted` carries one index
    (keep the list type in WIT for later multi-select).
  - Only allowed in `Command` / `Callback` context. Limit: 1 000 000 items.
  - Tests (in-editor): command opens a picker; `<ret>`, `<C-s>`, `<C-v>`,
    `<esc>` deliver the right results; preview doesn't crash on missing
    files; a second `show-picker` while one is open replaces it.
  - Done, except text previews: `preview::text` is accepted but not shown
    yet (the picker previews files and documents only). Answered plugin
    pickers are not kept for `space '`.
- [x] **M2.3 `push-items`.** Depends on: M2.2.
  - Keep `id → Injector` for open plugin pickers (drop on close). Lets a
    plugin `spawn` a program and stream its lines in as they arrive (with
    M1.2, add an optional `on-process-output(id, chunk)` callback, behind a
    `stream: bool` spawn option).
  - Done: `push-items` into an open picker. Streaming process output
    (`on-process-output`) needs a new API version and is left for later.
- [x] **M2.4 Live pickers.** Depends on: M2.2.
  - `with_dynamic_query`-backed pickers: a `live: bool` picker flag and an
    `on-picker-query(id, query)` callback that may call `push-items` /
    `set-items`. Debounce like the global search. Enables live grep.
  - Done: `live: bool`, `on-picker-query`, `set-items`; the typed text goes
    to a hidden primary column (the picker needs one filtering column). See
    `examples/finder` (`finder/grep`).
- [x] **M2.5 Docs.** Depends on: M1.2, M2.2.
  - No example plugins (see Scope). The guide has a picker snippet (spawn
    `rg --files`, pick, open with the chosen split).

## M3. Popups, prompts and messages

- [x] **M3.1 `show-popup`.** A `ui::Popup` with markdown (`ui::Markdown`), at
  the cursor or centered, closed with Esc; returns an id; optional
  `on-popup-close`. For hover-like info, help and plugin output.
  - Done as fire-and-forget: markdown at the cursor, closed by the next key
    (no id, no close callback yet).
- [x] **M3.2 `prompt` and `confirm`.** Text input with an optional
  completion list (`ui::Prompt`) → `on-prompt-done(id, option<string>)`;
  yes/no confirm built on the same thing. Enables "rename file", "new note
  title", "git commit message".
  - Done: `show-prompt` (message, initial text, fuzzy completions) →
    `on-prompt-done`. No separate `confirm`: a prompt with completions
    `["yes", "no"]` does it.
- [x] **M3.3 Notifications.** Status messages with severity and a
  `log`-backed `:plugin-log <name>` buffer showing a plugin's log lines.
  - Done: `:plugin-log <plugin>` (last 1000 lines). Status messages already
    have `set-status` / `set-error`.

## M4. File system access

- [x] **M4.1 `filesystem` permission.** WASI preopens rather than new host
  functions, so `std::fs` works in Rust plugins unchanged: `read` → the
  workspace root (and the plugin's own data dir), read-only; `write` →
  read-write. Granted in `plugins.toml` like `process`. The plugin's data dir
  `<data>/plugins/<name>/` is always read-write.
  - Done: `allow-files = "read" | "write"`. Preopens use the host paths as
    guest paths, so paths from `document-path` / `workspace-root` work with
    `std::fs`. They're fixed at load time: after `:cd`, `:plugin-reload`.
- [x] **M4.2 Workspace info.** Host functions `workspace-root`,
  `config-dir`, `cwd`, and the open documents' modified state.
- [x] **M4.3 Docs.** A guide section on file access.

## M5. Decorations

Each one needs a small hook in `helix-view`/`helix-term` rendering, so design
the hook first and keep it generic.

- [ ] **M5.1 Highlights.** Plugin-provided highlight ranges per document
  (scope name + char range), cleared by id. Use the existing overlay
  highlight mechanism.
- [ ] **M5.2 Virtual text.** Inline and end-of-line annotations via
  `TextAnnotations` (like inlay hints) for git blame, test results, values.
- [ ] **M5.3 Gutter signs.** A plugin gutter column (one char + style per
  line), opt-in through `editor.gutters`.
- [ ] **M5.4 Statusline segment.** `statusline` element
  `plugin:<name>/<segment>` whose text the plugin sets with
  `set-statusline(segment, text)`.

## M6. Events and key bindings

- [x] **M6.1 More events.** `document-save` (post-save), `document-close`,
  `selection-change` (debounced), `post-command(name)`, `focus-gained`/`lost`,
  `editor-idle`. Each one is a variant in `event-kind`, a hook in
  `plugins::register_hooks` and an export.
  - Done: save, close and selection change, in a new `events` export
    interface. Still open: `post-command`, focus and idle (see TODO list).
- [ ] **M6.2 Default key bindings from plugins.** `[keys]` in `plugin.toml`,
  merged *under* the user's config (the user always wins) and listed in
  `:plugin-list`. Turn off per plugin with `keys = false` in `plugins.toml`.
- [ ] **M6.3 Plugin commands as first-class commands.** Optional:
  `MappableCommand` support so `space t = "@todo/toggle"` works and plugin
  commands show up in the command palette with their docs. Only if it stays a
  small upstream diff; `:plugin-run` already works for bindings.

## M7. Long-running work

- [ ] **M7.1 Background calls.** Run an export on a worker thread without
  editor access (pure computation on data the plugin was given), with a
  longer limit, and deliver the result as a callback. Lets plugins do heavy
  parsing without freezing the editor and without WASI 0.3.
- [ ] **M7.2 WASI 0.3 async.** Once the toolchain pin allows a Wasmtime with
  stable component-model async, allow `async` exports. Research task first:
  report which Wasmtime/Rust versions are needed.

## M8. Distribution

- [ ] **M8.1 `:plugin-install <git-url | path>`.** Clone into
  `<config>/plugins/<name>`, use a committed `plugin.wasm` (no building on the
  user's machine by default), show the requested permissions and ask before
  enabling. `:plugin-update`, `:plugin-remove`.
- [ ] **M8.2 Lock file.** `plugins.lock` with the commit and a SHA-256 of the
  wasm; refuse a changed wasm until reinstalled.
- [ ] **M8.3 Template and other languages.** A `cargo generate` template;
  documented examples for Go (TinyGo), JavaScript (`jco componentize`) and
  Python (`componentize-py`) built from the same WIT.

## M9. Hardening and docs

- [x] **M9.1 Fuzz the host functions** (offsets, overlapping edits, huge
  strings, invalid UTF-8 from processes) with `cargo fuzz` or proptest.
- [x] **M9.2 Benchmarks.** Startup with 0/5/20 plugins (warm and cold cache),
  event dispatch cost while typing. Budget: under 5 ms of startup per cached
  plugin.
- [ ] **M9.3 Docs.** Keep `helix-plugin/README.md` the single guide (user
  setup, every permission, every host function with an example); a book
  page linking to it.

## Later: APIs for file-manager style plugins

Pure won't ship a file manager (see Scope), but users should be able to write
one. A persistent tree panel (nvim-tree / oil) needs plugin-defined views that
upstream Helix doesn't have. After M2 (pickers), M3 (prompts), M4 (file
access) and M5 (decorations) the remaining piece is either an "oil-style"
editable buffer API (a scratch document the plugin fills and reads back on
save) or a `ui` component API. Start with a research task: compare both and
estimate the upstream diff.

---

## Suggested hand-off batches

Tasks in one batch touch the same code, so give each batch to one agent.

1. **Done:** M0, M1, M2, M3.1–M3.2, M4, M6.1 (see `plugin-progress.md`).
2. **Open, no owner review needed:** M3.3, M9.1–M9.3 and the small TODOs
   below.
3. **Needs the owner first:** M5, M6.2, M6.3, M7, M8 and the file-manager
   APIs.

## Future TODOs (not yet milestones)

Small, self-contained; each one is a good single task.

- [x] **Text previews in pickers** (`preview::text`): `PathOrId::Text`,
  cached as a document; SDK `Item::with_text_preview`.
- [ ] **Streaming program output**: `on-process-output(id, chunk)` behind a
  `stream: bool` spawn option, so a picker can fill as `rg` runs. Needs
  the next API version once 0.2 is released.
- [ ] **Popup ids and `on-popup-close`**, and popups placed centered.
- [ ] **More events**: `post-command(name)`, `focus-gained` / `focus-lost`,
  `editor-idle`.
- [ ] **Open with a split from plugins**: `open-file` only replaces the
  view; `examples/finder` works around it with `:vsplit` through
  `run-command`. Add an `open-action` (and a position) to `open-file`.
- [ ] **Per-program process prompt** (owner's open question): ask once the
  first time a plugin runs a program, remember the answer.
- [ ] **Stale preopens**: plugins see the workspace from load time; reload
  their WASI context (or warn) after `:cd`.
- [x] **Parallel compilation on a cold cache**, and plugins load in the
  background after startup (see "Performance" in `plugin-progress.md`).
- [ ] **A `plugins` cargo feature** (on by default) to build Pure without
  Wasmtime at all, for a smaller binary and faster builds.
- [ ] **Fuzz the WASM boundary itself** (M9.1 covers the host's validation
  with quickcheck): a test plugin that sends random edits, ranges and
  picker data through the real ABI.
- [x] **Caching `find_workspace`** in `plugins::snapshot`.

## Guidelines for agents (lessons from earlier work)

- **Push often.** An earlier agent built M1.1–M1.3 and lost all of it
  because it was never pushed before its session ended. Commit and push
  every finished task to your branch.
- **API versions:** 0.2 may still change until it is merged to `master`.
  After that it is frozen (never edit a released WIT); new functions go in
  a new version. Add new *exports* in separate interfaces (like `callbacks`
  and `events`): the SDK's `register_plugin!` implements `plugin` for both
  API versions, so changing `plugin` breaks the 0.1 ABI.
- **Fixtures:** after changing the WIT, the SDK or a guest, run
  `helix-plugin/build-plugins.sh` and commit the `.wasm` files. A stale
  fixture fails at load time with "a matching implementation was not found
  in the linker". The script strips local paths; don't commit components
  built any other way.
- **Don't commit environment notes** (machine paths, proxy issues,
  `/workspace/...`, lockfiles of the standalone SDK).
- **In-editor tests** (`helix-term/tests/test/plugins.rs`):
  - call `app.editor.reset_idle_timer()` before
    `run_event_loop_until_idle` when there's no input, or it waits forever
    (use the `deliver_events` and `wait_for_status` helpers);
  - only the first `Application` of a test gets the job queue (a
    `OnceLock`): results of programs, pickers and prompts never reach a
    second one. Reload plugins with `load_with_settings` on the same app
    instead;
  - key notation: `<gt>` / `<lt>` for `>` / `<`.
- **Pickers:** don't keep an `Injector` around (the picker shows
  "(running)" while one lives); get one from the picker when needed. The
  picker needs at least one filtering column.
- **Manual checks:** the integration build overwrites `target/debug/pure`
  with a test build: run `cargo build` before trying it in tmux. Use a
  scratch `XDG_CONFIG_HOME` with `helix/plugins/` and `helix/plugins.toml`.
  Don't use `pkill -f` / `pgrep -f` (they match your own shell).
- **Disk space** is tight: `CARGO_INCREMENTAL=0`, and delete
  `target/debug/incremental` when a build fails with "no space left".
- **Scope:** API only in the editor; trial plugins stay in
  `helix-plugin/examples/`.
- **Performance (owner's rule: plugins must never make Pure slower than
  Helix):**
  - nothing plugin-related may run on the startup path or per keystroke
    unless a plugin asked for it: plugins load in the background
    (`plugins::load_in_background`), editor hooks check the `WANTED`
    bitmask (no lock) before doing anything;
  - no background wake-ups while idle: the epoch ticker only runs during a
    plugin call (`Ticker::busy`);
  - anything that walks the file system or scans documents per call must be
    cached (see `plugins::workspace`);
  - measure with `cargo test -p helix-plugin --release -- --ignored
    --nocapture benchmarks` and update the numbers in `plugin-progress.md`.
