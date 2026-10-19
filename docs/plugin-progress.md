# Plugin roadmap progress

Task list and design: [`plugin-roadmap.md`](plugin-roadmap.md),
[`plugin-processes-and-pickers.md`](plugin-processes-and-pickers.md).

## Status

- Done: M0 (API 0.2 groundwork), M1 (processes and the terminal), M2
  (pickers, including live pickers), M3.1–M3.2 (popups, prompts), M4 (file
  access, workspace info), M6.1 (save, close and selection events).
- Also done: M3.3 (`:plugin-log`), M9.1 (property tests), M9.2
  (benchmarks).
- Next: M9.3 and the "Future TODOs" in the roadmap.
- Needs the owner's design review first: M5, M6.2, M6.3, M7, M8 and the APIs
  for file-manager style plugins.
- Owner's rules: nothing built into the editor (the built-in yazi command was
  removed); plugins written to try the API are kept as examples in
  `helix-plugin/examples/` (`text-tools`, `yazi`, `finder`).

## Open questions for the owner

- Should Pure also ask once per program the first time a plugin runs it, on
  top of `allow-process` in `plugins.toml`? Only the config grant exists now.
- Should a plugin's `run-command` (API 0.1's `run-commands` permission, which
  can run `:sh`, `:write`, ...) also need the `process` grant? Unchanged for
  now.

## M0 API 0.2 groundwork (done)

- `wit/v0.1/` (byte-identical to the original API) and `wit/v0.2/` (adds
  request ids, `process`, `ui` and the `callbacks` export).
- `helix-plugin/src/api.rs`: bindings for both worlds; both call the same
  `State` methods. `api` in `plugin.toml` picks the world.
- Request ids from one process-wide counter (never reused, even across
  reloads), one-shot callbacks with the command time limit and a fresh
  snapshot, and call contexts: UI only from commands and callbacks.
- SDK 0.2 (`helix-plugin-api`): closures for results, exported callbacks;
  the `api-v0-1` feature builds the old ABI. `text-tools` builds unchanged.
- Test plugin `tests/plugins/testplugin2`. The frozen
  `tests/fixtures/text-tools-v0.1.wasm` checks 0.1 compatibility.
- Review fixes: `register_request` is test-only (`#[doc(hidden)]`), the SDK
  lockfile is not committed, and `build-plugins.sh` strips local paths from
  the components.

## M1 Processes and the terminal (done)

- `process` permission plus the user's grant: `allow-process = true` or a
  list of programs under the plugin's table in `plugins.toml`
  (`ProcessGrant`, `PluginHost::set_process_grant`). Shown by `:plugin-list`.
  Programs can't be started from `init`.
- `spawn`: `helix-term/src/plugins/process.rs` runs the program without a
  shell, with stdin, cwd, a timeout (default 30 s, max 10 min), 16 MiB
  output caps and lossy UTF-8. If a child keeps the pipes open after the
  program exits, output collection stops after 500 ms. `kill` and reloading
  plugins kill programs. At most 16 at once per plugin.
- `run-interactive`: hands the terminal over like `:run-interactive`;
  `{chooser}` becomes a fresh private (0600) temp file whose contents are
  returned and which is then deleted; `wait-for-key` works like `space z`.
- Results reach the plugin through a new job callback that also gets
  `Jobs` (`job::Callback::EditorCompositorJobs`), so callbacks can run
  editor commands.
- Tests: host tests for permission, grant, allow-list, the process limit,
  kill and delivery; executor unit tests (exit code, both streams, stdin,
  cwd, lossy UTF-8, timeout with a child holding the pipes, kill, missing
  program, chooser files); an in-editor test running `sh` through a plugin
  with an allow-list.

## M2 Pickers (done except M2.4)

- M2.1: `Picker::with_on_close` (cancel hook, once, never after an accept,
  including Alt-Enter) and `Picker::not_reopenable` (answered plugin
  pickers aren't kept for `space '`).
- M2.2: `helix-term/src/plugins/picker.rs` shows a plugin's picker with up
  to 8 columns, file previews and the initial item; Enter, `C-s`, `C-v`
  and Esc report the original index and the split, or a cancel. Text
  previews are not shown yet.
- M2.3: `push-items` (SDK `editor::push_items`) adds items to an open
  picker; the SDK keeps the items so the callback gets pushed ones too.
- Tests: host tests for pickers and `push-items`; in-editor tests for every
  key, filtering, and that `space '` doesn't reopen an answered picker.

## M2.4, M3, M4, M6.1 (done)

API 0.2 was extended in place (it is not released yet):

- Prompts (`show-prompt` → `on-prompt-done`) and popups (`show-popup`,
  markdown at the cursor). `helix-term/src/plugins/prompt.rs`.
- Live pickers: `live` pickers send what the user types to the plugin
  (`on-picker-query`, debounced, through a hidden primary column) and the
  plugin answers with `set-items`. The query callback doesn't complete the
  request. Pickers now make an injector when they need one (a kept one
  showed "(running)" forever).
- File access: `filesystem` permission plus `allow-files = "read" |
  "write"`; the workspace is preopened at its real path. Every plugin gets a
  read-write `data-dir` (`<data dir>/plugins/<name>`). Settings and grants
  are passed at load time (`PluginSettings`, `PluginHost::load_with`,
  `set_dirs`).
- `workspace-root`, `cwd`, `config-dir`, `data-dir`, `document-modified`.
- Events `document-save` (from `Application::handle_document_write`),
  `document-close`, `selection-change` (debounced), exported by 0.2 plugins
  in a separate `events` interface.
- SDK: `prompt`, `popup`, `pick_live` (handler gets the picker id and may
  answer later), `set_items`, `Item::new`, `Item::with_file_preview`,
  handlers for the new events.
- Examples: `examples/yazi` (chooser through `run-interactive`) and
  `examples/finder` (files, live grep, git branches), checked by hand in
  tmux.
- Tests: 52 host tests, 26 in-editor plugin tests (prompts, live picker,
  the three events), full suite as in the roadmap's "Commands".

## M3.3, M9.1, M9.2 (done)

- `:plugin-log <plugin>` opens the plugin's last 1000 `log` lines in a new
  buffer (`PluginHost::log_lines`).
- Property tests (quickcheck, `helix-plugin/src/test.rs`, `mod properties`):
  plugin edits apply like a simple model or are rejected exactly when they
  overlap or are out of range; selections and `text-range` never panic.
- Benchmarks: `cargo test -p helix-plugin --release -- --ignored --nocapture
  benchmarks`. On this machine (4 cores), loading `text-tools`:

  | Plugins | Cold cache | Warm cache |
  | --- | --- | --- |
  | 1 | 48 ms | 1.5 ms |
  | 5 | 218 ms | 5.3 ms |
  | 20 | 936 ms | 19.7 ms |

  Dispatching one event to one plugin (with a 10 000-line document): 0.05 ms.
  Warm-cache startup is within the 5 ms per plugin budget; cold-cache
  compilation is sequential (a TODO).
