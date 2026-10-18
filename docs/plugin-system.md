# Plugin system design (WebAssembly)

Status: **proposal** — nothing here is implemented yet.

This fork wants plugins: small pieces of user code that add commands, react to
editor events and change buffers, without rebuilding Helix. This document picks
a technology, describes the architecture and splits the work into phases that
can each be merged and used on their own.

## Context

- **Upstream Helix chose Steel** (a Scheme) for its plugin system. The work lives
  in the draft PR [helix-editor/helix#8675], still under active development in
  October 2026 and not merged. Maintainers consider the language choice settled,
  so a WASM system will stay a fork-only feature. That makes **keeping the
  plugin code isolated** (its own crate, few touch points in existing files) a
  hard requirement, so that merging upstream keeps being cheap.
- Editors and tools that already ship WASM plugins and are good references:
  - **Zed**: extensions written in Rust, compiled to WASM components and run in
    Wasmtime; the API is defined in WIT and versioned per release
    (`wit/since_v0.x`) so old extensions keep working.
  - **Zellij**: WASI plugins with a `load` / `update(event)` / `render` life
    cycle, event subscriptions and a permission system (`ReadApplicationState`,
    `RunCommand`, `OpenFiles`, ...) that the user approves.
- WebAssembly tooling is now mature enough: **WASI 0.3** (released 2026-06-11)
  adds native `async`, `stream<T>` and `future<T>` to the component model, and
  Wasmtime supports it by default.

## Options

| Option | Pros | Cons |
| --- | --- | --- |
| **WASM component model + WIT** (Wasmtime) | Sandboxed; any language with a WASM target (Rust, Go, C, Python via componentize-py, JS via jco); typed API generated from one WIT file; versionable; async with WASI 0.3; same approach as Zed | Biggest dependency (Wasmtime + Cranelift: longer builds and a few MB more binary); plugins need a compile step; API design work up front |
| WASM core modules + Extism | Simple bytes-in/bytes-out ABI, many host/guest SDKs | Untyped (JSON/bytes everywhere), no versioned interface, still Wasmtime underneath |
| Steel (upstream) | Will be the upstream API; live scripting, no compile step | Scheme only; no sandbox; tracking an unmerged, moving PR |
| Lua (mlua) | Familiar from Neovim, tiny, fast to embed | No sandbox by default; dynamic API; diverges from both upstream and the WASM world |

**Recommendation: WASM components with a WIT-defined API, hosted with
Wasmtime.** It is the only option that gives sandboxing, a typed and versioned
API and freedom of language at the same time, and it can be built as an
isolated crate behind a cargo feature.

## Architecture

```
            ┌──────────────── helix-term ────────────────┐
 keymap ──► │ MappableCommand::Plugin ─┐                  │
 events ──► │ helix_event hooks ───────┼─► helix-plugin ──┼──► Wasmtime ──► plugin.wasm
            │ (DocumentDidChange, ...) │   (host)         │   (one Store per plugin)
            │ Editor / Compositor  ◄───┘  host functions  │
            └─────────────────────────────────────────────┘
```

### New crate: `helix-plugin`

Owns everything plugin related so the rest of the code base only gets small
hooks:

- a `wasmtime::Engine` (shared) and one `Store<PluginState>` per plugin, with
  `bindgen!`-generated bindings for the WIT world;
- loading, the compilation cache (`Engine::precompile_component` / Wasmtime's
  cache) so startup does not recompile plugins every time;
- dispatching events and commands to plugins;
- the host side of the API (the functions plugins import);
- permissions and resource limits.

`helix-term` depends on it behind a `plugins` cargo feature (on by default in
this fork), so it can be switched off if it ever gets in the way of an upstream
merge.

### The API, in WIT

One WIT package, versioned like Zed's (`wit/v0.1/`, `wit/v0.2/`, ...). The host
keeps supporting old versions so plugins don't break on every release. A first
version could look like this:

```wit
package helix:plugin@0.1.0;

interface types {
  type document-id = u32;
  record position { line: u32, column: u32 }
  record range { anchor: u32, head: u32 }        // char offsets, like Helix
  enum mode { normal, insert, select }
  record edit { from: u32, to: u32, text: string }
}

/// Functions the editor provides to plugins.
interface editor {
  use types.{document-id, range, mode, edit};

  current-document: func() -> option<document-id>;
  document-path: func(doc: document-id) -> option<string>;
  document-language: func(doc: document-id) -> option<string>;
  text: func(doc: document-id) -> string;
  text-range: func(doc: document-id, from: u32, to: u32) -> string;
  selections: func(doc: document-id) -> list<range>;
  mode: func() -> mode;

  /// Edits are applied as one transaction (one undo step) after the plugin
  /// call returns.
  apply-edits: func(doc: document-id, edits: list<edit>);
  set-selections: func(doc: document-id, ranges: list<range>);

  set-status: func(message: string);
  set-error: func(message: string);
  open-file: func(path: string);
  /// Run any built-in or typable command, e.g. "goto_line_end" or ":w".
  run-command: func(command: string) -> result<_, string>;
}

/// Functions a plugin provides to the editor.
interface plugin {
  use types.{document-id, mode};

  record command-info { name: string, doc: string }
  enum event-kind { document-open, document-change, document-save, document-close,
                    selection-change, mode-switch }

  /// Called once after loading. Returns the commands the plugin adds and the
  /// events it wants to hear about.
  init: func(config: string) -> tuple<list<command-info>, list<event-kind>>;
  run-command: func(name: string, args: list<string>) -> result<_, string>;

  on-document-open: func(doc: document-id);
  on-document-change: func(doc: document-id);
  on-document-save: func(doc: document-id);
  on-mode-switch: func(old: mode, new: mode);
}

world helix-plugin {
  import editor;
  export plugin;
}
```

A Rust SDK crate (`helix-plugin-api`) wraps the generated bindings with an
ergonomic trait and a `register_plugin!` macro, the way `zed_extension_api` and
`zellij-tile` do. Other languages use the same WIT file with their own
generators.

### Calling into plugins and accessing the editor

The editor state (`Editor`, `Compositor`) lives on the main thread and is
mutably borrowed while commands and hooks run. The simplest correct model:

1. A plugin call happens **synchronously on the main thread**, from a command
   or from an event hook.
2. For the duration of the call, the store's `PluginState` gets temporary
   access to the editor (a scoped borrow handed to the call). Host functions
   read straight from it, so `text()` and `selections()` are cheap and always
   current.
3. Writes (`apply-edits`, `set-selections`, `run-command`, `open-file`) are
   **queued** in `PluginState` and applied after the call returns, as one
   transaction per document. That keeps edits atomic (one undo step), avoids
   re-entrancy (a plugin command triggering events that call plugins again) and
   keeps borrow handling simple.
4. Each call has a **time budget**: Wasmtime's epoch interruption stops a call
   that runs too long (say 50 ms for event handlers, longer for explicit
   commands). A trap or timeout disables the plugin and shows an error; it
   never takes the editor down.

Long-running work (calling external programs, network, heavy analysis) comes
later through WASI 0.3 async: plugin exports become `async`, the host drives the
future from a tokio task, and results come back to the main thread through
Helix's existing job queue (`job::dispatch`).

### Hooks into the existing code

Kept small on purpose:

- **Events**: `helix_event::register_hook!` for `DocumentDidOpen`,
  `DocumentDidChange`, `DocumentDidClose`, `SelectionDidChange`, `OnModeSwitch`
  and `PostCommand` (all already exist), plus a save event. Only plugins that
  subscribed to an event are called, and change events are debounced so typing
  doesn't call plugins on every key.
- **Commands**: plugin commands become typable commands (`:todo-toggle`) and a
  new `MappableCommand::Plugin { plugin, name }` variant so they can be bound in
  `config.toml`:

  ```toml
  [keys.normal.space]
  t = "@plugin:todo/toggle"
  ```

- **Startup**: load plugins in `Application::new` after the config is read.

### Configuration, installation and permissions

Plugins live in `~/.config/helix/plugins/<name>/` with the component and a
manifest:

```toml
# plugin.toml
name = "todo"
version = "0.1.0"
api = "0.1"
description = "Toggle TODO checkboxes"
permissions = ["read-buffers", "edit-buffers"]
```

and are enabled in `config.toml`, which is also where per-plugin settings go
(passed to `init` as a string):

```toml
[plugins.todo]
enabled = true
config = { done-marker = "x" }
```

Permissions are **capabilities**: a host function that wasn't granted is simply
not linked, so the plugin fails to load instead of failing later. Planned
capabilities: `read-buffers`, `edit-buffers`, `run-commands`, `open-files`,
`filesystem` (WASI preopens limited to the workspace), `process` (spawn
programs) and `network`. When a plugin asks for more than the user granted,
Helix shows what it wants and refuses to load it until the config grants it.

Memory is capped with a Wasmtime `ResourceLimiter` per store.

## Phases

Each phase ends with something usable and merged.

1. **Spike** (a few days): add Wasmtime behind the `plugins` feature, load one
   hard-coded component, call `init`, have it call `set-status`. Measure build
   time, binary size and startup time with the compilation cache.
2. **Commands and buffers**: the `editor` and `plugin` interfaces above minus
   events; `MappableCommand::Plugin`; loading from `~/.config/helix/plugins`;
   the Rust SDK; two example plugins (sort lines, toggle TODO). Errors and
   timeouts disable the plugin with a message.
3. **Events, permissions, limits**: event subscriptions with debouncing, the
   manifest and permission checks, memory and time limits, `:plugin-list`,
   `:plugin-reload`.
4. **Async and UI**: WASI 0.3 async exports for long-running work; UI APIs
   (pickers with plugin-provided items, popups, status line segments, virtual
   text/annotations).
5. **Distribution**: `:plugin-install <git url>`, API version negotiation
   (`wit/v0.x` kept side by side), docs and a template repository.

## Risks and open questions

- **Build time and size**: Wasmtime with Cranelift adds noticeable compile time
  and a few MB to the binary. Measure in phase 1; the `plugins` feature lets it
  be turned off.
- **Upstream divergence**: if upstream merges Steel, keep both. The hook points
  (events, `MappableCommand`) are the same ones Steel uses, so the two
  shouldn't conflict much, but `MappableCommand` changes are the most likely
  merge conflicts.
- **API stability**: start small. Exposing too much of Helix's internals would
  make every upstream refactor a breaking change for plugins (the same concern
  upstream maintainers raised about exposing `Editor`/`Context` in the Steel
  PR).
- **Event volume**: `DocumentDidChange` fires on every keystroke; debounce and
  only call subscribers.
- **Re-entrancy**: a plugin command that runs other commands or edits text can
  trigger events that call plugins again; queueing writes and not dispatching
  events to the plugin that caused them avoids loops.

[helix-editor/helix#8675]: https://github.com/helix-editor/helix/pull/8675
