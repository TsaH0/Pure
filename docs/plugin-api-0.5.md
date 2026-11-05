# Plugin API 0.5: evaluation and design

This document evaluates Pure's plugin system against eight capability areas,
and specifies what API 0.5 adds for each. It was the contract for the 0.5
implementation; the status below says what landed and where it differs.

API 0.5 is a new WIT world (`helix-plugin/wit/v0.5`). Released worlds may
gain imports, never exports, and most of what follows needs new exports
(key callbacks, hooks, completion and lifecycle events, service calls). 0.5
contains everything in 0.4. Pure keeps loading 0.1–0.4 components unchanged.

## Status (2026-10-26): all eight areas implemented

Each line names the tests that cover it (host tests in
`helix-plugin/src/test.rs`, editor tests in `helix-term/tests/test/plugins.rs`).

| Area | Status | Differs from the design below |
| --- | --- | --- |
| 1. Keymaps | Done: `keymaps.set/unset/list`, `hooks.on-key`, layers, pass-through, document/language scope, `:plugin-keys` (`api_05_key_bindings_*`, `plugin_key_bindings_*`) | — |
| 2. Completion | Done: `completion-menu` state and control, lifecycle events with close reasons, `completion-keys` (`plugins_navigate_and_confirm_the_completion_menu`, `aborting_drops_the_preview_and_closing_keeps_it`, `leaving_insert_mode_reports_why_the_menu_closed`, `completion_keys_drive_the_menu`) | `close` keeps the preview, `abort` drops it |
| 3. Events | Done: filtered subscriptions, pre-command and pre-save hooks, buffer/view enter and leave, plugin-change, config-change, shutdown (`api_05_subscriptions_*`, `api_05_hooks_*`, `pre_command_hooks_*`, `pre_save_hooks_*`, `plugins_hear_about_focus_plugins_and_shutdown`) | — |
| 4. Commands and state | Done: `commands.list/execute/batch/set-mode`, `state.views/ui` (`api_05_commands_*`, `plugins_run_native_and_typable_commands_with_results`, `plugin_batches_stop_at_the_first_failure`, `plugins_switch_modes_and_see_views_and_ui`) | Commands need `run-commands`, and `allow-process` for those that start programs. `state.ui` is as of the last frame |
| 5. Services | Done: register/discover/call, callers, versions, cycles and depth, unavailable providers, `dependencies` and `optional-dependencies` (`api_05_services_*`, `api_05_service_chains_*`, `api_05_unavailable_providers_fail_calls`, `api_05_dependencies_load_first`, `plugins_call_each_others_services`) | — |
| 6. UI | Done: floats, marks, virtual lines, patched highlight and annotation layers (`plugin_floats_*`, `plugin_marks_follow_edits`, `api_05_marks_*`, `plugin_virtual_lines_*`, `plugins_patch_parts_of_decoration_layers`) | Virtual lines go below a line only (Helix's line annotations have no rows above) |
| 7. Async | Done: sessions, progress, file watching, LSP requests (`plugin_sessions_*`, `plugin_session_input_is_bounded`, `plugin_progress_*`, `plugins_watch_paths_they_may_read`, `plugin_lsp_requests_need_the_grant`) | `lsp` also needs the user's `allow-lsp`; the editor's lifecycle methods (`initialize`, `shutdown`, `exit`, `$/…`) are refused. The statusline shows plugins' text by default now, so progress is visible |
| 8. Configuration and tooling | Done: settings schemas, `config-change` on `:config-reload`, `:plugin-doctor`, `:plugin-trace`, 0.5 capabilities (`api_05_settings_*`, `api_05_plugins_hear_about_settings_changes`, `api_05_tracing_and_the_doctor_report`, `lens_example_*`, `plugin_settings_schemas_are_checked_at_load`, `plugin_doctor_and_trace_report_what_plugins_do`) | Setting types are `boolean`, `string`, `integer`, `number`, `string-list` and `enum`. A grant, key or `enabled` change reloads the plugins instead. The doctor (not `:plugin-list`) shows settings. Tracing logs each call into the plugin with its time and number of actions, not each host function |

Added after the eight areas: **syntax trees** (`syntax` interface;
`api_05_syntax_*`, `plugins_query_the_syntax_tree`). Plugins run
tree-sitter queries over a document's tree, find the node at a position and
walk to parents, children and siblings, and read the editor's own query
files (`tags.scm`, `textobjects.scm`...). The snapshot carries a copy of the
root tree (a reference count, no copying) only while an API 0.5 plugin is
loaded. Injected languages aren't covered. The `outline` example uses it:
symbols come from `tags.scm` (about 80 languages), with the line-by-line
scan as the fallback.

Also added (2026-10-30), so plugins can change built-in behaviour:
`commands.feed-keys` (keys as if typed, through every keymap; an
unfinished command is abandoned; program-starting commands the keys run
need `allow-process`; keys being fed can't feed more), `keymaps.next-key`
(the next key goes to `on-key`, as after `f`), and `state.command-register`
/ `command-count` for `pre-command` hooks (`plugins_feed_keys_and_take_the_next_key`,
`pre_command_hooks_see_and_may_cancel_commands`). The `find-char` and
`vim-registers` examples use them (`find_char_example_replaces_f_and_t`,
`vim_registers_example_fills_numbered_registers`).

The `lens` example (`helix-plugin/examples/lens`) uses most of it: a
settings schema, `config-change`, a key bound to a closure, filtered
subscriptions, virtual lines and a service.

## Ground rules that apply to every area

- **No WASM on the hot path unless a plugin asked for it.** A key press
  enters a plugin only when the key is bound to that plugin's callback.
  Events are delivered only to subscribers, and the editor checks an atomic
  bitmask before building anything. No new per-keystroke event exists.
- **Calls are synchronous against a snapshot; changes apply afterwards, in
  order.** This keeps the model from 0.1: a plugin call reads a snapshot
  of the editor and records actions; the editor applies them after the call
  returns, in the order they were recorded. Events raised while actions are
  applied (for example a buffer change from a plugin's edit) are queued and
  delivered after the current call, never inside it. No plugin call ever
  runs inside another plugin call, so there is no reentrancy.
- **Two kinds of hooks.** *Observational events* (`on-event`) report what
  happened; they can't change it and are delivered after the fact, possibly
  coalesced. *Synchronous hooks* (`on-key`, `on-pre-command`, `on-pre-save`)
  run before the editor acts and return an outcome that the editor obeys
  (handled/pass-through, continue/cancel). Hooks have small time budgets
  (below) and are only called for what the plugin scoped them to.
- **Budgets.** `init` 5 s, commands and callbacks 2 s, events 250 ms, key
  callbacks and hooks 100 ms, service calls 250 ms. A plugin that runs out
  of time or memory, or traps, is disabled until `:plugin-reload`; for a
  hook this means "continue", for a key it means "pass-through", so a
  broken plugin can't swallow keys or block saving.
- **Permissions stay per plugin.** Every action is checked against the
  permissions of the plugin that records it. Nothing a plugin does on behalf
  of another (services, below) borrows the other plugin's permissions.

## 1. Programmable input and keymaps

### Before 0.5

| Capability | State |
| --- | --- |
| Plugin commands as key targets | `[keys]` in `plugin.toml`, values are the plugin's own commands; they only fill keys nothing else binds |
| Buffer scope | `[keys.buffer.<mode>]`: only in the plugin's own buffers, above everything else |
| Precedence | implicit: buffer keys > user + native (merged) > plugin defaults |
| Pass-through | none: a plugin command always consumes the key |
| Interception | none |
| Input injection | `run-command` runs one command (typable or static) |
| Recursion | `:plugin-run` depth limit (4) |
| Conflict diagnostics | none |

### 0.5 design

`keymaps.set(binding)` binds a key sequence to a **callback** of the plugin:
the editor calls the export `on-key(name, keys, count)` and the plugin answers
`handled` or `pass-through`. Bindings are scoped by mode, and optionally to
one document or one language. Static `[keys]` keep working unchanged.

**Precedence**, from first to last (explicit and documented):

1. **Editor UI that has focus**: prompts, pickers, popups, the completion
   menu, a focused panel or float. They get keys before any keymap.
2. **Plugin buffer keys** (`[keys.buffer]`) in the plugin's own buffers.
3. **`override` layer**: plugin bindings above the user's. A plugin can only
   use it if the user grants `allow-key-override = true` in `plugins.toml`;
   this is the scoped pre-dispatch interception.
4. **User mappings** (`config.toml` `[keys]`).
5. **`plugin` layer**: plugin bindings above Pure's defaults but below the
   user's (a key counts as the user's when the user's keymap differs from
   the default at that key sequence).
6. **Native defaults.**
7. **`fallback` layer** and static `[keys]`: only keys nothing else binds.

Within a layer, the plugin that set the binding last wins; the losers are
reported as conflicts.

**Pass-through and fallback.** When `on-key` returns `pass-through` (or the
plugin is disabled, or exceeds its budget), the editor runs what the same
key sequence is bound to with that binding removed, which is the next layer
down. Count and register are kept.

**Structured dispatch instead of key injection.** Plugins don't inject keys.
`commands.execute(name, args, count)` runs a command by name (static,
typable or another plugin's) with typed arguments and reports the result
(section 4). This can't recurse through the keymap.

**Recursion and budgets.** A key callback may execute commands; those don't
consult the keymap, so they can't re-enter `on-key`. Plugin commands that
run plugin commands are limited to a depth of 4 (as before).

**Diagnostics.** `:plugin-keys [mode]` lists every plugin binding with its
layer and scope, and the conflicts: which binding shadows which (user over
plugin, plugin over native default, plugin over plugin). `keymaps.list()`
gives a plugin the same information about its own bindings.

## 2. Completion control

### Before 0.5

Plugins can be completion *sources* (`on-completion` + `complete`,
`snippets.complete`). They can't see or drive the menu.

### 0.5 design

The source API stays as it is; the **menu controller** is a separate
interface, `completion-menu`:

| Function | Meaning |
| --- | --- |
| `state() -> option<menu-state>` | Whether the menu is visible, its items (labels, at most 1000), the selected index and whether a preview is applied |
| `select(index)` / `next()` / `previous()` | Navigate; like the menu keys, this previews the item in the document |
| `confirm()` | Accept the selected item (or the first): its full edit, additional edits and snippet |
| `abort()` | Undo the preview, restoring the typed text, and close |
| `close()` | Close without touching the text: the previewed text stays as typed text |

They are actions, applied after the call in order with other actions, so
`abort` followed by `commands.execute("normal_mode")` safely aborts and then
leaves insert mode. Confirming and aborting go through the menu's own code,
so the preview rollback, undo history, cursor and LSP synchronisation stay
exactly what pressing the keys does.

**Events** (`on-event`, subscribed with `completion`): `opened(count)`,
`updated(count)`, `confirmed(label)`, `closed(reason)` with reasons
`aborted`, `confirmed`, `dismissed` (typed away, empty), `mode-change` and
`replaced` (a new request).

**Configurable menu keys.** `[keys.completion]` in `config.toml` maps keys
to menu actions (`confirm`, `abort`, `close`, `next`, `previous`,
`page-up`, `page-down`, `normal-mode`), the last one being abort followed by
leaving insert mode in one key press. The default keeps Helix's keys plus
`C-c` = `normal-mode`.

## 3. Events and lifecycle

### Before 0.5

`document-open/change/save/close`, `selection-change` (debounced),
`mode-switch`, `post-command`, `focus-gained/lost`, `editor-idle`,
`completion` (source) and `buffer-write`; static subscription in `init`.

### 0.5 design

New events (observational, `on-event`):

| Event | When |
| --- | --- |
| `buffer-enter(doc, view)` / `buffer-leave(doc, view)` | The focused view shows another document |
| `view-enter(view)` / `view-leave(view)` | Focus moves between views (splits) |
| `completion(...)` | Menu lifecycle (section 2) |
| `config-change` | The user's settings for this plugin changed (section 8) |
| `plugin-change(plugin, state)` | Another plugin loaded, unloaded or was disabled |
| `file-change(id, paths)` | A watched path changed (section 7) |
| `shutdown` | The editor is quitting; last chance to save state (budget 250 ms) |

New synchronous hooks:

| Hook | Outcome | Scope |
| --- | --- | --- |
| `on-pre-command(command)` | `continue` or `cancel` | Only the command names the plugin subscribed to |
| `on-pre-save(doc)` | `continue` or `cancel(reason)`; edits it makes are applied before writing | Documents matching the subscription filter |

**Subscriptions** become dynamic and filterable: `events.subscribe(kind,
filter) -> id` and `events.unsubscribe(id)`. Filters: language, path glob,
document id, command names. Static `init` subscriptions keep working.

**Ordering.** For one editor action, hooks run before it, in plugin load
order; the first `cancel` stops it (later plugins aren't asked). Events run
after it, in plugin load order. Events caused by applying a plugin's own
actions are not reported back to that plugin (as before for changes).
`shutdown` runs once, before the editor exits, for every subscriber.

## 4. Commands and editor state

### Before 0.5

`run-command(":write")` or a static command name, fire-and-forget; queries
for documents, text, selections, mode, registers, workspace.

### 0.5 design

- `commands.list()`: every static, typable and plugin command with its doc
  and kind.
- `commands.execute(name, args, count) -> request-id`: typed arguments (no
  command-line parsing, no `%sh` expansion), the result delivered with
  `on-command-done(id, result)`: the command's error, or ok.
- `editor.set-mode(mode)`: explicit mode transitions (`normal`, `insert`,
  `select`), through the same code as the keys.
- `state.views()` (id, document, area, visible lines, focus),
  `state.ui()` (completion menu visible, prompt/picker/popup open, focused
  panel, pending keys).
- `commands.batch(steps) -> request-id`: steps run in order after the call;
  the first failing step stops the batch and the rest are reported as
  skipped (`on-batch-done(id, results)`). **No rollback**: steps that ran
  stay done; edits within one batch are separate undo steps.

**Where things are allowed** (unchanged rules, now documented in the WIT):

| | init | commands, callbacks, key callbacks | events, hooks |
| --- | --- | --- | --- |
| Reads | yes | yes | yes |
| Edits, selections, decorations, diagnostics | yes | yes | yes |
| UI (pickers, prompts, popups), `run-command`, `execute`, `set-mode`, menu control | no | yes | no (`on-pre-save` may edit) |
| Programs, timers, file ops, background tasks, services | no | yes | yes |

## 5. Plugin-to-plugin composition

### Before 0.5

None.

### 0.5 design

- `services.register(name, version)` from `init`: a service named
  `<plugin>.<name>` (namespaced by the host, so one plugin can't take over
  another's names) with a semantic version.
- `services.discover(name) -> list<service-info>`.
- `services.call(service, version-req, method, payload) -> request-id`: the
  payload must be valid JSON (checked by the host, at most 1 MiB). The
  host calls the provider's export `on-service-call(caller, service, method,
  payload) -> result<string, string>` after the caller's call returns, then
  delivers `on-service-reply(id, result)` to the caller.
- **Permissions don't travel.** The provider runs with its own permissions
  and is told who called; the caller gets only the returned JSON. A
  provider that acts on a caller's behalf must decide that it is willing to
  (it knows the caller); `register` can restrict callers to a list.
- **Cycles and depth.** Calls are queued, never nested. A call made while
  serving another inherits its chain; a chain longer than 4 or one that
  would call a plugin already in it fails with an error.
- **Unavailable providers.** A call to a disabled or unloaded provider fails
  at once with an error; pending calls fail when it goes.
- **Dependencies.** `dependencies = ["other"]` and `optional-dependencies`
  in `plugin.toml`: load order follows them; a missing required dependency
  is a load error; cycles are refused. `plugin-change` events report
  providers coming and going.

## 6. UI extensibility

### Compared with Neovim

| Neovim | Pure before 0.5 | 0.5 |
| --- | --- | --- |
| Buffers | owned buffers (`open-buffer`), scratch | unchanged |
| Windows (splits) | open with hsplit/vsplit | queries (`state.views`), focus events |
| Floating windows | popups (markdown, no keys), pickers, prompts | **floats**: interactive item lists with keys, focus, size, position |
| Extmarks | decorations follow edits, but no queryable anchors | **marks**: tracked positions with gravity, queryable |
| Virtual text | inline annotations, overlays, conceals, line backgrounds | + **virtual lines** below a line |
| Incremental decoration updates | replace a whole layer | + **patch** a range of a layer |

All of these are editor-managed primitives: plugins describe content, the
editor renders, handles keys, scrolls, resizes and cleans up (a plugin's
floats, marks and layers go when it is disabled or reloaded). Plugins never
get raw drawing access.

**Editor-core work done since:** folding (`folds`, native `zc`/`zo`),
embedded terminals (`terminal`, native `:terminal`), split control and
plugin-drawn buffers (`windows`), statuslines, tab lines and winbars
(`bars`), the undo tree (`history`), mouse events (`mouse`), the debugger
(`debug`) and injected languages' trees (`syntax.layers`). Still not
there: multi-line virtual blocks between lines with their own scrolling,
and plugin-provided tree-sitter grammars (native parsers can't be
sandboxed; plugins can run their own queries over the editor's trees and
decorate with the results). Virtual lines are the subset Helix's line
annotations support: whole rows attached to a document line, which
scrolling and cursor movement account for.

## 7. Async work and external integrations

### Before 0.5

`process.spawn` (with streaming stdout), `run-interactive`, timers,
background tasks in a second instance, `files.apply`, `requests.cancel`,
revisions for stale results, local HTTP listeners.

### 0.5 design

| Feature | Design |
| --- | --- |
| Process sessions | `sessions.start(argv, options)`: stdout streams as before; `sessions.write(id, data)` and `close-stdin(id)`. **Backpressure**: at most 1 MiB of unwritten input per session; `write` fails with "busy" until it drains |
| Progress | `progress.begin(title) -> token`, `report(token, message, percent)`, `end(token)`; shown in the statusline like a language server's progress |
| Stale results | `revisions` (0.4); `requests.cancel`; every async result names its request id |
| File watching | `watch.add(path, recursive) -> id` (needs the `filesystem` permission and a path inside the granted roots); changes arrive as `file-change(id, paths)` events, coalesced (100 ms); event-driven (inotify/FSEvents through `notify`), no polling |
| LSP | `lsp.servers(doc)` and `lsp.request(doc, server, method, params) -> request-id` with `on-lsp-reply`; needs the `lsp` permission. Only requests (no notifications that change server state), methods the server advertises |
| Network | Added (2026-10-29) as the `http` interface: `http.fetch(request) -> request-id`, the response through `on-service-reply` (0.5 can't gain exports). Needs the `network` permission and the user's `allow-network = ["api.github.com", "*.example.org"]`. HTTPS only, no redirects, no user info in URLs, editor-set headers (`Host`, `Content-Length`...) refused, 4 MiB bodies, 16 MiB responses, 1–120 s timeouts, 8 in flight. The editor sends it with the system's `curl` (`-q`, a cleaned environment, `--proto =https`, `--globoff`) and pins it with `--resolve` to an address it checked is public (no loopback, private, link-local, CGNAT, NAT64...), so DNS can't redirect it to a local service. Through a configured proxy, a name the editor can't resolve is left to the proxy. Tests: `api_05_http_*`, `plugin_https_requests_never_reach_local_addresses`, `helix_plugin::http` unit tests |

## 8. Configuration and developer tooling

### Before 0.5

Settings passed to `init` as JSON, unchecked; `requests.capabilities`;
`:plugin-list`, `:plugin-log`, `CallTiming`; host tests with real
components, in-editor integration tests.

### 0.5 design

- **Settings schemas** in `plugin.toml`:
  ```toml
  [settings.width]
  type = "integer"      # boolean, string, integer, number, string-list, enum
  default = 80
  min = 20
  max = 200
  description = "Width of the panel"
  ```
  The host validates `plugins.toml` against it before `init` (unknown keys,
  wrong types, out of range are load errors that name the key), fills in
  defaults, and `:plugin-list` shows the settings.
- **Change notifications**: when `plugins.toml` changes only a plugin's
  settings (not its grants), `:config-reload` delivers `config-change` with
  the new settings instead of reloading the plugin.
- **Capabilities and versions**: `requests.capabilities` lists every
  optional feature; the WIT version policy is in the README.
- **Tooling**: `:plugin-keys` (bindings, layers, conflicts), `:plugin-trace
  <plugin> on|off` (every host call and callback with timings, to
  `:plugin-log`), `:plugin-doctor` (load errors, missing grants for
  requested permissions, unknown settings, conflicts, slow calls).
- **Tests**: host tests load real components for every interface; in-editor
  tests drive the real editor; the test plugin exercises every 0.5 call.
